use std::sync::Arc;

use async_trait::async_trait;

use agent_contracts::backend::capability::exec::ExecRequest;
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

    fn format_output(
        result: &agent_contracts::backend::capability::exec::ExecResult,
        spill_dir: Option<&std::path::Path>,
    ) -> BashOutput {
        let (stdout, stdout_truncated) = window_stream(&result.stdout, spill_dir);
        let (stderr, stderr_truncated) = window_stream(&result.stderr, spill_dir);

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

fn window_stream(bytes: &[u8], spill_dir: Option<&std::path::Path>) -> (String, bool) {
    let normalize = |slice: &[u8]| String::from_utf8_lossy(slice).replace("\r\n", "\n");
    if bytes.len() <= MAX_OUTPUT_BYTES_PER_STREAM {
        return (normalize(bytes), false);
    }
    let head_bytes = MAX_OUTPUT_BYTES_PER_STREAM / 2;
    let tail_bytes = MAX_OUTPUT_BYTES_PER_STREAM - head_bytes;
    let elided = bytes.len() - head_bytes - tail_bytes;
    let head = normalize(&bytes[..head_bytes]);
    let tail = normalize(&bytes[bytes.len() - tail_bytes..]);
    let retrieval = match spill_full_output(bytes, spill_dir) {
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

const SPILL_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);
const SPILL_SESSION_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

fn spill_full_output(bytes: &[u8], spill_dir: Option<&std::path::Path>) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let dir = spill_dir?;
    if let Some(root) = dir.parent() {
        sweep_stale_spills(root);
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{:016x}.txt", hasher.finish()));
    std::fs::write(&path, bytes).ok()?;
    enforce_session_budget(dir, SPILL_SESSION_BUDGET_BYTES, &path);
    Some(path.to_string_lossy().into_owned())
}

fn sweep_stale_spills(root: &std::path::Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let now = std::time::SystemTime::now();
        let Ok(sessions) = std::fs::read_dir(root) else {
            return;
        };
        for session in sessions.flatten() {
            let dir = session.path();
            let Ok(files) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut remaining = 0usize;
            for f in files.flatten() {
                let stale = f
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .map(|age| age > SPILL_TTL)
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(f.path());
                } else {
                    remaining += 1;
                }
            }
            if remaining == 0 {
                let _ = std::fs::remove_dir(&dir);
            }
        }
    });
}

fn enforce_session_budget(dir: &std::path::Path, budget: u64, keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
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
        if path == keep {
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

fn bash_spill_dir(runtime: &dyn RuntimeView) -> std::path::PathBuf {
    let metadata = runtime.agent_context().metadata();
    let session = metadata
        .session_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| metadata.agent_id.clone());
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".xiaoo")
        .join("bash-output")
        .join(sanitize_session(&session))
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

        let spill_dir = bash_spill_dir(runtime);
        let output = Self::format_output(&result, Some(spill_dir.as_path()));

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
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn make_spill(dir: &Path, name: &str, size: u64, mtime: SystemTime) -> PathBuf {
        let path = dir.join(name);
        let f = std::fs::File::create(&path).expect("create spill");
        f.set_len(size).expect("set_len");
        f.set_modified(mtime).expect("set_modified");
        path
    }

    #[test]
    fn budget_evicts_oldest_first_keeping_freshest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let old = make_spill(dir.path(), "old.txt", 60, now - Duration::from_secs(30));
        let mid = make_spill(dir.path(), "mid.txt", 60, now - Duration::from_secs(20));
        let fresh = make_spill(dir.path(), "fresh.txt", 60, now - Duration::from_secs(10));

        enforce_session_budget(dir.path(), 100, &fresh);

        assert!(!old.exists(), "oldest spill should be evicted");
        assert!(!mid.exists(), "next-oldest spill should be evicted");
        assert!(fresh.exists(), "freshest spill must survive");
    }

    #[test]
    fn budget_never_evicts_kept_file_even_when_oldest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let keep = make_spill(dir.path(), "keep.txt", 60, now - Duration::from_secs(30));
        let a = make_spill(dir.path(), "a.txt", 60, now - Duration::from_secs(20));
        let b = make_spill(dir.path(), "b.txt", 60, now - Duration::from_secs(10));

        enforce_session_budget(dir.path(), 100, &keep);

        assert!(keep.exists(), "kept file must never be evicted");
        assert!(
            !a.exists(),
            "non-kept spill should be evicted to honor budget"
        );
        assert!(
            !b.exists(),
            "non-kept spill should be evicted to honor budget"
        );
    }

    #[test]
    fn budget_under_limit_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let a = make_spill(dir.path(), "a.txt", 30, now);
        let b = make_spill(dir.path(), "b.txt", 30, now);

        enforce_session_budget(dir.path(), 100, &a);

        assert!(a.exists(), "nothing should be evicted under budget");
        assert!(b.exists(), "nothing should be evicted under budget");
    }
}
