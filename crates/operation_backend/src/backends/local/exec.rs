#[cfg(unix)]
#[allow(unused_imports)]
use std::os::unix::process::CommandExt;

use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    OperationError,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

pub(crate) struct LocalExec {
    _state: Arc<LocalBackendState>,
}

impl LocalExec {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationExec for LocalExec {
    fn default_shell(&self) -> Option<&str> {
        self._state.default_shell.as_deref()
    }

    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        let command_spec = build_command_spec(&request)?;
        let command_cwd = if let Some(cwd) = request.cwd.as_ref() {
            let cwd = self._state.backend_path_to_host(cwd)?;
            self._state.policy.check_exec_cwd(cwd.as_path())?;
            self._state.ensure_directory(cwd.as_path())?;
            Some(cwd)
        } else if self._state.policy.requires_exec_cwd() {
            let cwd = self._state.workspace_root_host.clone();
            self._state.policy.check_exec_cwd(cwd.as_path())?;
            self._state.ensure_directory(cwd.as_path())?;
            Some(cwd)
        } else {
            None
        };

        let mut command =
            command_from_spec(command_spec, &self._state.policy, command_cwd.as_deref());

        if let Some(env_vars) = &request.env {
            for (k, v) in env_vars {
                command.env(k, v);
            }
        }

        if let Some(cwd) = command_cwd {
            command.current_dir(cwd);
        }

        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;

        #[cfg(unix)]
        let pgid = child.id().unwrap_or(0) as i32;
        #[cfg(unix)]
        if pgid > 0 {
            crate::process_group::register_pgid(pgid);
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "failed to capture stdout".to_string(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "failed to capture stderr".to_string(),
            })?;

        // Drain stdout/stderr into shared buffers while the child runs. We use
        // incremental chunked reads into an `Arc<Mutex<Vec<u8>>>` (rather than
        // `read_to_end` into a task-owned `Vec`) so that any output captured
        // *before* a reader task is aborted is preserved. This matters when a
        // background process spawned by the command (e.g. `foo &`) inherits the
        // bash stdout pipe: that process keeps the pipe's write end open, so
        // `read_to_end` would never observe EOF and the reader task would hang
        // forever. The shared buffer lets us return the output captured up to
        // that point instead of dropping it (or deadlocking) when we abort the
        // reader below.
        let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut stdout_task = spawn_pipe_drainer(stdout, stdout_buf.clone());
        let mut stderr_task = spawn_pipe_drainer(stderr, stderr_buf.clone());

        let (exit_code, timed_out) = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
                Ok(status) => {
                    let status = status.map_err(|error| OperationError::ExecutionFailed {
                        message: error.to_string(),
                    })?;
                    #[cfg(unix)]
                    crate::process_group::unregister_pgid(pgid);
                    (status.code(), false)
                }
                Err(_) => {
                    #[cfg(unix)]
                    {
                        crate::process_group::send_sigterm_to_group(pgid);
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        crate::process_group::send_sigkill_to_group(pgid);
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill().await;
                    }
                    let _ = child.wait().await;
                    (None, true)
                }
            }
        } else {
            let status = child
                .wait()
                .await
                .map_err(|error| OperationError::ExecutionFailed {
                    message: error.to_string(),
                })?;
            #[cfg(unix)]
            crate::process_group::unregister_pgid(pgid);
            (status.code(), false)
        };

        // After the child has exited (or the overall timeout fired and we
        // killed the process group), give the reader tasks a short, bounded
        // grace period to drain any remaining buffered output and reach EOF.
        // On timeout, the group kill ensures lingering background processes
        // release the pipe, so EOF arrives quickly. On a normal exit where a
        // background process still holds the pipe, the grace bounds the wait so
        // we never hang forever; whatever was captured into the shared buffers
        // is returned as partial output. Previously the reader `.await`s here
        // were unbounded, which deadlocked in the lingering-background-process
        // case described above.
        let _ = timeout(DRAIN_GRACE, async {
            let _ = tokio::join!(&mut stdout_task, &mut stderr_task);
        })
        .await;
        // Stop the reader tasks regardless of whether the grace drain finished,
        // so they cannot keep a background writer's pipe open or outlive the
        // request. Any output already captured remains in the shared buffers.
        stdout_task.abort();
        stderr_task.abort();

        let stdout = std::mem::take(&mut *stdout_buf.lock().unwrap_or_else(|e| e.into_inner()));
        let stderr = std::mem::take(&mut *stderr_buf.lock().unwrap_or_else(|e| e.into_inner()));

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
            timed_out,
        })
    }
}

/// Maximum time to wait for the stdout/stderr reader tasks to finish after the
/// child process has exited (or been killed).
///
/// This is a safety bound only: in the common case the readers observe EOF as
/// soon as the child exits and complete immediately, adding no latency. The
/// bound exists for the case where a background process spawned by the command
/// (`foo &`) inherited the pipe and keeps its write end open, which would
/// otherwise cause the readers to block forever waiting for an EOF that never
/// arrives. One second is enough to drain anything already buffered in the
/// kernel pipe (typically 64 KiB) while still failing fast when the pipe is
/// genuinely held open by a lingering background writer.
const DRAIN_GRACE: Duration = Duration::from_millis(1000);

/// Spawn a task that copies a child process pipe (`ChildStdout`/
/// `ChildStderr`) into a shared buffer using incremental reads.
///
/// Using incremental chunked reads into an `Arc<Mutex<Vec<u8>>>` (rather than
/// `read_to_end` into a task-owned `Vec`) means that if the task is aborted
/// while still running, any output already captured survives in the shared
/// buffer and can be returned as partial output. With `read_to_end` the buffer
/// is owned by the task and is lost on abort, which would discard the entire
/// captured stream when a lingering background writer forces us to abort.
fn spawn_pipe_drainer<R>(reader: R, sink: Arc<Mutex<Vec<u8>>>) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
                    guard.extend_from_slice(&chunk[..n]);
                }
                Err(_) => break,
            }
        }
    })
}

#[cfg(all(test, unix))]
fn test_workspace_root(prefix: &str, name: &str) -> std::path::PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let root = std::env::temp_dir().join(format!("{prefix}{name}-{}-{millis}", std::process::id()));
    let _ = std::fs::remove_dir_all(root.as_path());
    std::fs::create_dir_all(root.join("workspace")).unwrap();
    root
}

struct LocalCommandSpec {
    program: String,
    args: Vec<String>,
}

fn build_command_spec(request: &ExecRequest) -> Result<LocalCommandSpec, OperationError> {
    if request.command.trim().is_empty() {
        return Err(OperationError::ExecutionFailed {
            message: "command cannot be empty".to_string(),
        });
    }

    if let Some(shell) = request.shell.as_deref() {
        if !request.args.is_empty() {
            return Err(OperationError::Unsupported {
                message: "shell execution does not support args".to_string(),
            });
        }
        return Ok(LocalCommandSpec {
            program: shell.to_string(),
            args: vec!["-c".to_string(), request.command.clone()],
        });
    }

    Ok(LocalCommandSpec {
        program: request.command.clone(),
        args: request.args.clone(),
    })
}

fn command_from_spec(
    spec: LocalCommandSpec,
    policy: &crate::backends::local::policy::LocalBackendPolicy,
    cwd: Option<&std::path::Path>,
) -> Command {
    if let Some(profile) = policy.seatbelt_profile() {
        let mut command = Command::new("sandbox-exec");
        command.arg("-p").arg(profile.to_profile_text());
        command.arg(spec.program);
        command.args(spec.args);
        return command;
    }

    if let Some(cwd) = cwd {
        if let Some(args) = policy.bubblewrap_args(cwd) {
            let mut command = Command::new("bwrap");
            command.args(args);
            command.arg(spec.program);
            command.args(spec.args);
            return command;
        }
    }

    let mut command = Command::new(spec.program);
    command.args(spec.args);
    command
}

#[cfg(all(test, target_os = "linux"))]
mod linux_bubblewrap_tests {
    use super::*;
    use crate::backends::local::factory::local_backend_with_isolation;
    use agent_contracts::backend::{
        BackendPath, SandboxPermissionCapability, SandboxPermissionGrantRequest,
        SandboxPermissionScope, SandboxPolicyDenial,
    };
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn bubblewrap_exec_enforces_filesystem_policy() {
        if !has_bwrap() {
            return;
        }
        // Serialize against the process-group unit tests: `exec` registers real
        // child pgids in the shared global registry, and `kill_all_process_groups`
        // would otherwise clear it (and signal our children) mid-test.
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = super::test_workspace_root("xiaoo-bubblewrap-", "fs");
        let workspace = root.join("workspace");
        let writable = workspace.join("tmp");
        let outside = root.join("outside");
        std::fs::create_dir_all(writable.as_path()).unwrap();
        std::fs::create_dir_all(outside.as_path()).unwrap();
        std::fs::write(workspace.join("readable.txt"), b"visible").unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

        let backend = bubblewrap_backend(workspace.clone(), writable.clone(), false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let read = runtime.block_on(exec_bash(backend.as_ref(), &workspace, "cat readable.txt"));
        assert_eq!(read.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(read.stdout.as_slice()), "visible");

        let write = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            "printf ok > tmp/out.txt",
        ));
        assert_eq!(write.exit_code, Some(0));
        assert_eq!(
            std::fs::read_to_string(writable.join("out.txt")).unwrap(),
            "ok"
        );

        let denied_write = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            "printf no > blocked.txt",
        ));
        assert_ne!(denied_write.exit_code, Some(0));
        assert!(!workspace.join("blocked.txt").exists());

        let outside_path = outside.join("secret.txt");
        let denied_read = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            format!("cat {}", shell_quote(outside_path.as_path())).as_str(),
        ));
        assert_ne!(denied_read.exit_code, Some(0));
        assert!(!String::from_utf8_lossy(denied_read.stdout.as_slice()).contains("secret"));

        backend
            .permission_control()
            .unwrap()
            .grant(SandboxPermissionGrantRequest {
                denial: SandboxPolicyDenial {
                    backend_id: backend.backend_id().to_string(),
                    isolation: "linux_bubblewrap".to_string(),
                    operation: "test".to_string(),
                    capability: SandboxPermissionCapability::Read,
                    path: outside_path.display().to_string(),
                },
                scope: SandboxPermissionScope::Session,
            })
            .unwrap();

        let granted_read = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            format!("cat {}", shell_quote(outside_path.as_path())).as_str(),
        ));
        assert_eq!(granted_read.exit_code, Some(0));
        assert_eq!(
            String::from_utf8_lossy(granted_read.stdout.as_slice()),
            "secret"
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    #[test]
    fn bubblewrap_exec_can_unshare_network() {
        if !has_bwrap() {
            return;
        }
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = super::test_workspace_root("xiaoo-bubblewrap-", "net");
        let workspace = root.join("workspace");
        let writable = workspace.join("tmp");
        std::fs::create_dir_all(writable.as_path()).unwrap();

        let backend = bubblewrap_backend(workspace.clone(), writable, false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(exec_bash(backend.as_ref(), &workspace, "cat /proc/net/dev"));

        assert_eq!(result.exit_code, Some(0));
        let interfaces = String::from_utf8_lossy(result.stdout.as_slice())
            .lines()
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(name, _)| name.trim().to_string())
            })
            .collect::<Vec<_>>();
        assert!(
            interfaces.iter().all(|name| name == "lo"),
            "unexpected interfaces in unshared network namespace: {interfaces:?}"
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    fn bubblewrap_backend(
        workspace: PathBuf,
        writable: PathBuf,
        allow_network: bool,
    ) -> std::sync::Arc<dyn agent_contracts::backend::OperationBackend> {
        local_backend_with_isolation(
            workspace.clone(),
            None,
            Some(writable.clone()),
            None,
            Some(json!({
                "kind": "linux_bubblewrap",
                "allow_network": allow_network,
                "readable_roots": [workspace.to_string_lossy().to_string()],
                "writable_roots": [writable.to_string_lossy().to_string()]
            })),
        )
        .unwrap()
    }

    async fn exec_bash(
        backend: &dyn agent_contracts::backend::OperationBackend,
        cwd: &Path,
        command: &str,
    ) -> ExecResult {
        backend
            .exec()
            .exec(ExecRequest {
                command: command.to_string(),
                args: vec![],
                shell: Some("bash".to_string()),
                cwd: Some(BackendPath(cwd.to_string_lossy().into_owned())),
                timeout_ms: Some(5_000),
                env: None,
            })
            .await
            .unwrap()
    }

    fn has_bwrap() -> bool {
        std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("bwrap").is_file()))
            .unwrap_or(false)
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
}

#[cfg(all(test, unix))]
mod pipe_drain_tests {
    use crate::backends::local::factory::local_backend;
    use agent_contracts::backend::{capability::exec::ExecRequest, BackendPath};
    use std::time::{Duration, Instant};

    // Regression test for the stdout-pipe deadlock: a background process spawned
    // by the script (`sleep 3 &`) inherits bash's piped stdout and keeps its
    // write end open for longer than the exec timeout. The old implementation
    // only wrapped `child.wait()` in the timeout; bash exits almost instantly,
    // then the subsequent unbounded `stdout_task.await` blocked until the
    // background `sleep` finally exited (~3s) and released the pipe. With the
    // fix, the post-exit drain is bounded by `DRAIN_GRACE` (1s), so exec returns
    // promptly with the partial output captured before the drain gave up.
    #[test]
    fn exec_returns_promptly_when_background_process_holds_stdout_pipe() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = super::test_workspace_root("xiaoo-pipe-", "pipe");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let command = "echo main-output; sleep 3 &";
        let start = Instant::now();
        let result = runtime.block_on(async {
            backend
                .exec()
                .exec(ExecRequest {
                    command: command.to_string(),
                    args: vec![],
                    shell: Some("bash".to_string()),
                    cwd: Some(BackendPath(workspace.to_string_lossy().into_owned())),
                    timeout_ms: Some(2_000),
                    env: None,
                })
                .await
                .unwrap()
        });
        let elapsed = start.elapsed();

        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out, "should not hit the overall timeout");
        let out = String::from_utf8_lossy(result.stdout.as_slice());
        assert!(out.contains("main-output"), "captured stdout was: {out:?}");
        // The background `sleep 3` keeps the pipe open for 3s. The fix must
        // return within the ~1s drain grace, well before that — the old impl
        // returned only after ~3s.
        assert!(
            elapsed < Duration::from_secs(2),
            "exec took {elapsed:?}, expected to return within the drain grace (~1s)"
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    // When the background writer releases the pipe on its own *before* the
    // drain grace elapses, the reader observes EOF normally and we capture the
    // full stream including the background writer's output.
    #[test]
    fn exec_captures_background_output_when_writer_exits_within_grace() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = super::test_workspace_root("xiaoo-pipe-", "bg");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // The background subshell writes three lines over ~0.3s then exits,
        // releasing the inherited stdout pipe. All of it should be captured.
        let command = "echo main; ( for i in 1 2 3; do echo bg-$i; sleep 0.1; done ) & echo done";
        let result = runtime.block_on(async {
            backend
                .exec()
                .exec(ExecRequest {
                    command: command.to_string(),
                    args: vec![],
                    shell: Some("bash".to_string()),
                    cwd: Some(BackendPath(workspace.to_string_lossy().into_owned())),
                    timeout_ms: Some(5_000),
                    env: None,
                })
                .await
                .unwrap()
        });

        assert_eq!(result.exit_code, Some(0));
        let out = String::from_utf8_lossy(result.stdout.as_slice());
        assert!(out.contains("main"), "stdout was: {out:?}");
        assert!(out.contains("done"), "stdout was: {out:?}");
        assert!(out.contains("bg-1"), "stdout was: {out:?}");
        assert!(out.contains("bg-3"), "stdout was: {out:?}");

        let _ = std::fs::remove_dir_all(root.as_path());
    }
}
