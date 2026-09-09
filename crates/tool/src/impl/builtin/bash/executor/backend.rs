use std::sync::Arc;

use async_trait::async_trait;

use agent_contracts::backend::capability::exec::ExecRequest;
use agent_contracts::backend::capability::filesystem::{
    TempPathKind, TempPathRequest, WriteBytesRequest, WriteMode,
};
use agent_contracts::backend::BackendPath;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::super::validation::backend as validation;
use super::super::validation::interactive;
use super::constants::{default_timeout_ms, MAX_OUTPUT_BYTES_PER_STREAM};
use super::input::BashInput;
use super::output::BashOutput;
use super::spec::BashToolSpec;

pub struct BashExecutor {
    spec: Arc<BashToolSpec>,
}

impl BashExecutor {
    pub fn new(spec: Arc<BashToolSpec>) -> Self {
        Self { spec }
    }

    async fn resolve_and_stat_cwd(
        cwd: Option<&str>,
        backend: &dyn agent_contracts::backend::OperationBackend,
    ) -> Result<Option<(BackendPath, agent_contracts::backend::PathStat)>, String> {
        let Some(cwd) = cwd else {
            return Ok(None);
        };

        let cwd_str = cwd.trim();

        let base = agent_contracts::backend::capability::path::ResolveBase::WorkspaceRoot;
        let resolved = backend
            .paths()
            .resolve_path(
                agent_contracts::backend::capability::path::ResolvePathRequest {
                    raw_path: cwd_str.to_string(),
                    base,
                },
            )
            .await
            .map_err(|e| format!("Failed to resolve cwd path: {}", e))?;

        let stat = backend
            .files()
            .stat(&resolved)
            .await
            .map_err(|e| format!("Failed to stat cwd path: {}", e))?;

        Ok(Some((resolved, stat)))
    }

    async fn format_output(
        backend: &dyn agent_contracts::backend::OperationBackend,
        result: &agent_contracts::backend::capability::exec::ExecResult,
        session: &str,
    ) -> BashOutput {
        let (stdout, stdout_truncated) = window_stream(backend, session, &result.stdout).await;
        let (stderr, stderr_truncated) = window_stream(backend, session, &result.stderr).await;

        BashOutput {
            stdout,
            stdout_truncated,
            stderr,
            stderr_truncated,
            exit_code: result.exit_code,
            interrupted: result.timed_out,
        }
    }
}

const SPILL_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);
const SPILL_SESSION_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

async fn window_stream(
    backend: &dyn agent_contracts::backend::OperationBackend,
    session: &str,
    bytes: &[u8],
) -> (String, bool) {
    let normalize = |slice: &[u8]| String::from_utf8_lossy(slice).replace("\r\n", "\n");
    if bytes.len() <= MAX_OUTPUT_BYTES_PER_STREAM {
        return (normalize(bytes), false);
    }
    let head_bytes = MAX_OUTPUT_BYTES_PER_STREAM / 2;
    let tail_bytes = MAX_OUTPUT_BYTES_PER_STREAM - head_bytes;
    let elided = bytes.len() - head_bytes - tail_bytes;
    let head = normalize(&bytes[..head_bytes]);
    let tail = normalize(&bytes[bytes.len() - tail_bytes..]);
    let retrieval = match spill_full_output(backend, session, bytes).await {
        Some(path) => format!(
            "the FULL output is saved at {path} — `grep`/`sed -n` it or read it to retrieve any region"
        ),
        None => format!(
            "re-run piping through `tail -c {tail_bytes}`, `sed -n 'A,Bp'`, or `grep <pattern>` to retrieve a specific region"
        ),
    };
    let marker = format!(
        "\n\n…[bash output truncated: {elided} bytes elided from the middle ({total} total). \
         Head and tail are shown; {retrieval}.]…\n\n",
        total = bytes.len(),
    );
    (format!("{head}{marker}{tail}"), true)
}

async fn spill_full_output(
    backend: &dyn agent_contracts::backend::OperationBackend,
    session: &str,
    bytes: &[u8],
) -> Option<BackendPath> {
    let temp = backend
        .files()
        .temp_path(TempPathRequest {
            kind: TempPathKind::File,
            preferred_parent: None,
            prefix: Some(format!(".xiaoo-bash-output-{}-", sanitize_session(session))),
            suffix: Some(".txt".to_string()),
        })
        .await
        .ok()?;
    backend
        .files()
        .write_bytes(WriteBytesRequest {
            path: temp.clone(),
            content: bytes.to_vec(),
            mode: WriteMode::Overwrite,
        })
        .await
        .ok()?;
    reclaim_host_spills(temp.native(), session);
    Some(temp)
}

/// Best-effort reclamation of spill files that live on the host (local
/// backend). E2B spill paths are sandbox paths that do not exist on the host,
/// so this is a no-op there and their `/tmp` contents are reclaimed when the
/// sandbox is destroyed. All errors are swallowed: cleanup must never block or
/// fail the bash result.
fn reclaim_host_spills(native_path: &str, session: &str) {
    let path = std::path::Path::new(native_path);
    if !path.try_exists().unwrap_or(false) {
        return;
    }
    let Some(dir) = path.parent() else {
        return;
    };
    sweep_stale_spills(dir);
    let prefix = format!(".xiaoo-bash-output-{}-", sanitize_session(session));
    if let Some(keep) = path.file_name() {
        enforce_session_budget(dir, &prefix, keep, SPILL_SESSION_BUDGET_BYTES);
    }
}

fn sweep_stale_spills(dir: &std::path::Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let now = std::time::SystemTime::now();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if !name.starts_with(".xiaoo-bash-output-") {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .map(|age| age > SPILL_TTL)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    });
}

fn enforce_session_budget(
    dir: &std::path::Path,
    prefix: &str,
    keep: &std::ffi::OsStr,
    budget: u64,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(prefix))
        })
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    if total <= budget {
        return;
    }
    files.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, len, path) in files {
        if total <= budget {
            break;
        }
        if path.file_name().is_some_and(|name| name == keep) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

fn sanitize_session(session: &str) -> String {
    let cleaned: String = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "session".to_string()
    } else {
        cleaned
    }
}

fn bash_spill_session(runtime: &dyn RuntimeView) -> String {
    let metadata = runtime.agent_context().metadata();
    let session = metadata
        .session_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| metadata.agent_id.clone());
    sanitize_session(&session)
}

impl Default for BashExecutor {
    fn default() -> Self {
        Self::new(Arc::new(BashToolSpec::new()))
    }
}

#[async_trait]
impl ToolExecutor for BashExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: BashInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "bash requires operation backend access, but none is configured"
                        .to_string(),
                },
            });
        }
        let backend = backend.unwrap();

        let validation_result = validation::validate_command(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let validation_result = interactive::validate_interactive_command(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Interactive command validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let validation_result = validation::validate_timeout(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let cwd = Self::resolve_and_stat_cwd(input.cwd.as_deref(), &*backend)
            .await
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        let cwd_path = if let Some((resolved, stat)) = cwd {
            let cwd_str = input.cwd.as_deref().unwrap_or_default();
            let validation_result = validation::validate_cwd_backend(cwd_str, &stat);
            if !validation_result.result {
                let error_message = validation_result
                    .message
                    .unwrap_or_else(|| "Validation failed".to_string());
                let error_code = validation_result.error_code.unwrap_or(0);
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!("[error_code={}] {}", error_code, error_message),
                    },
                });
            }
            Some(resolved)
        } else {
            None
        };

        let request = ExecRequest {
            command: input.command.clone(),
            args: vec![],
            shell: Some("bash".to_string()),
            cwd: cwd_path,
            timeout_ms: Some(input.timeout.unwrap_or_else(default_timeout_ms)),
            extra: call.extra.clone(),
            ..Default::default()
        };

        let result = backend.exec().exec(request).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Backend exec failed: {}", e),
            }
        })?;

        let session = bash_spill_session(runtime);
        let output = Self::format_output(&*backend, &result, &session).await;

        let serialized =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output: serialized },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_contracts::backend::capability::filesystem::ReadBytesRequest;
    use agent_contracts::backend::{OperationError, PathNamespace};

    #[tokio::test]
    async fn overflow_spills_via_backend_and_stays_readable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
            .expect("local backend");

        // Exceed the per-stream window so the full output must be spilled.
        let bytes = vec![b'x'; MAX_OUTPUT_BYTES_PER_STREAM + 10_000];
        let (text, truncated) = window_stream(&*backend, "test-session", &bytes).await;
        assert!(truncated);

        // The model is told where the full output lives, so it can read it back.
        let marker = "the FULL output is saved at ";
        let start = text
            .find(marker)
            .unwrap_or_else(|| panic!("expected spill marker in: {text}"))
            + marker.len();
        let spill = text[start..]
            .split('—')
            .next()
            .expect("spill path in message")
            .trim();
        assert!(!spill.is_empty());

        let full = backend
            .files()
            .read_bytes(ReadBytesRequest {
                path: BackendPath::from_raw(spill.to_string()),
            })
            .await
            .expect("spill file readable through the backend");
        assert_eq!(
            full, bytes,
            "full output must round-trip through the backend"
        );

        // A path claiming a different backend must be rejected so cross-backend
        // paths can never be interpreted inside this one.
        let foreign = BackendPath::new(
            "some-other-backend",
            PathNamespace::Temp,
            spill.to_string(),
            "spill.txt",
        );
        let denied = backend
            .files()
            .read_bytes(ReadBytesRequest { path: foreign })
            .await;
        assert!(
            matches!(denied, Err(OperationError::PermissionDenied { .. })),
            "expected PermissionDenied, got {denied:?}"
        );
    }

    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn make_spill(dir: &Path, name: &str, size: u64, mtime: SystemTime) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).expect("create spill");
        file.set_len(size).expect("set_len");
        file.set_modified(mtime).expect("set_modified");
        path
    }

    #[test]
    fn budget_evicts_oldest_first_keeping_freshest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-1.txt",
            60,
            now - Duration::from_secs(30),
        );
        make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-2.txt",
            60,
            now - Duration::from_secs(20),
        );
        let fresh = make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-3.txt",
            60,
            now - Duration::from_secs(10),
        );

        enforce_session_budget(
            dir.path(),
            ".xiaoo-bash-output-s-",
            std::ffi::OsStr::new(".xiaoo-bash-output-s-3.txt"),
            100,
        );

        assert!(
            !dir.path().join(".xiaoo-bash-output-s-1.txt").exists(),
            "oldest spill should be evicted"
        );
        assert!(
            !dir.path().join(".xiaoo-bash-output-s-2.txt").exists(),
            "next-oldest spill should be evicted"
        );
        assert!(fresh.exists(), "freshest spill must survive");
    }

    #[test]
    fn budget_never_evicts_kept_file_even_when_oldest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let keep = make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-1.txt",
            60,
            now - Duration::from_secs(30),
        );
        make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-2.txt",
            60,
            now - Duration::from_secs(20),
        );
        make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-3.txt",
            60,
            now - Duration::from_secs(10),
        );

        enforce_session_budget(
            dir.path(),
            ".xiaoo-bash-output-s-",
            std::ffi::OsStr::new(".xiaoo-bash-output-s-1.txt"),
            100,
        );

        assert!(keep.exists(), "kept file must never be evicted");
        assert!(
            !dir.path().join(".xiaoo-bash-output-s-2.txt").exists(),
            "non-kept spill should be evicted to honour budget"
        );
        assert!(
            !dir.path().join(".xiaoo-bash-output-s-3.txt").exists(),
            "non-kept spill should be evicted to honour budget"
        );
    }

    #[test]
    fn budget_under_limit_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let spill = make_spill(
            dir.path(),
            ".xiaoo-bash-output-s-1.txt",
            30,
            now - Duration::from_secs(20),
        );

        enforce_session_budget(
            dir.path(),
            ".xiaoo-bash-output-s-",
            std::ffi::OsStr::new(".xiaoo-bash-output-s-1.txt"),
            100,
        );

        assert!(spill.exists(), "no files should be evicted under budget");
    }

    #[test]
    fn reclaim_skips_non_host_paths() {
        // A sandbox-style path (e.g. E2B) does not exist on the host, so the
        // reclaimer must be a no-op and never inspect the filesystem.
        reclaim_host_spills("/tmp/.xiaoo-bash-output-e2b-1.txt", "e2b");
    }
}
