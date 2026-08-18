#[cfg(unix)]
#[allow(unused_imports)]
use std::os::unix::process::CommandExt;

use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::exec::exec_streaming_via_exec,
    capability::{exec::ExecRequest, exec::ExecResult, exec::LineSink, OperationExec},
    OperationError,
};
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
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
        let stream_auth = self._state.policy.requires_stdin();
        let stdin = if stream_auth {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        };
        let mut child = self.spawn_child(&request, stdin)?;

        #[cfg(unix)]
        let pgid = child.id().unwrap_or(0) as i32;
        #[cfg(unix)]
        if pgid > 0 {
            crate::process_group::register_pgid(pgid);
        }

        #[cfg(unix)]
        if stream_auth {
            tracing::info!(
                "dyn-sandbox streaming exec start: pgid={} timeout_ms={:?}",
                pgid,
                request.timeout_ms
            );
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| OperationError::ExecutionFailed {
                    message: "failed to capture stdin".to_string(),
                })?;
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
            return self
                .exec_linux_dynsandbox(request.timeout_ms, child, stdin, stdout, stderr, pgid)
                .await;
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
                    kill_process_group(pgid).await;
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
            ..Default::default()
        })
    }

    /// Streaming variant of [`Self::exec`]. Reads stdout line-by-line
    /// through `sink`; when the sink returns `false`, the child process
    /// group is killed. The dyn-sandbox path falls back to
    /// buffer-then-sink (AUTH stdin is incompatible with stdout streaming).
    async fn exec_streaming(
        &self,
        request: ExecRequest,
        sink: Arc<dyn LineSink>,
    ) -> Result<ExecResult, OperationError> {
        let stream_auth = self._state.policy.requires_stdin();
        if stream_auth {
            // dyn-sandbox doubles stdin as the AUTH control channel — can't
            // stream stdout independently. Fall back to buffer-then-sink via
            // the free function (NOT `OperationExec::exec_streaming(self, ...)`,
            // which would recurse into this override).
            return exec_streaming_via_exec(self, request, sink).await;
        }

        let mut child = self.spawn_child(&request, std::process::Stdio::null())?;

        #[cfg(unix)]
        let pgid = child.id().unwrap_or(0) as i32;
        #[cfg(not(unix))]
        let pgid: i32 = 0;
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

        let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut stderr_task = spawn_pipe_drainer(stderr, stderr_buf.clone());

        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        let mut stopped_early = false;
        let line_loop = async {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if !sink.on_line(&line) {
                            stopped_early = true;
                            break;
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(_) => break,   // pipe error — child likely dead
                }
            }
        };

        let (exit_code, timed_out) = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), line_loop).await {
                Ok(_) => {
                    if stopped_early {
                        kill_process_group(pgid).await;
                    }
                    let status = reap_child(&mut child, pgid).await;
                    #[cfg(unix)]
                    crate::process_group::unregister_pgid(pgid);
                    (status.ok().and_then(|s| s.code()), false)
                }
                Err(_) => {
                    kill_process_group(pgid).await;
                    let _ = child.wait().await;
                    #[cfg(unix)]
                    crate::process_group::unregister_pgid(pgid);
                    (None, true)
                }
            }
        } else {
            line_loop.await;
            if stopped_early {
                kill_process_group(pgid).await;
            }
            let status = reap_child(&mut child, pgid).await;
            #[cfg(unix)]
            crate::process_group::unregister_pgid(pgid);
            (status.ok().and_then(|s| s.code()), false)
        };

        let _ = timeout(DRAIN_GRACE, &mut stderr_task).await;
        stderr_task.abort();

        let stderr = std::mem::take(&mut *stderr_buf.lock().unwrap_or_else(|e| e.into_inner()));

        Ok(ExecResult {
            stdout: Vec::new(),
            stderr,
            exit_code,
            timed_out,
            stopped_early,
        })
    }
}

/// Kill the child's process group. On Unix: SIGTERM, brief grace, SIGKILL.
/// On non-Unix: no-op (caller must `child.kill()` separately).
async fn kill_process_group(pgid: i32) {
    #[cfg(unix)]
    {
        crate::process_group::send_sigterm_to_group(pgid);
        tokio::time::sleep(Duration::from_millis(300)).await;
        crate::process_group::send_sigkill_to_group(pgid);
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
    }
}

/// Reap the child with a bounded wait. If it doesn't exit within
/// `DRAIN_GRACE` (e.g. a background process holds stdout open), kill
/// the process group and reap unconditionally.
async fn reap_child(
    child: &mut tokio::process::Child,
    pgid: i32,
) -> std::io::Result<std::process::ExitStatus> {
    match timeout(DRAIN_GRACE, child.wait()).await {
        Ok(s) => s,
        Err(_) => {
            #[cfg(unix)]
            kill_process_group(pgid).await;
            #[cfg(not(unix))]
            {
                let _ = child.kill().await;
            }
            child.wait().await
        }
    }
}

/// A dyn-sandbox `AUTH_REQ:filename:resolved` event decoded from stderr.
struct AuthEvent {
    filename: String,
    path: String,
}

impl LocalExec {
    /// Build the command (spec, cwd, env, stdio, process group) and spawn
    /// the child. Shared by `exec` and `exec_streaming` to avoid divergence.
    fn spawn_child(
        &self,
        request: &ExecRequest,
        stdin: std::process::Stdio,
    ) -> Result<tokio::process::Child, OperationError> {
        let command_spec = build_command_spec(request)?;
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
        let mut command = command_from_spec(
            request,
            command_spec,
            &self._state.policy,
            command_cwd.as_deref(),
        );
        if let Some(env_vars) = &request.env {
            for (k, v) in env_vars {
                command.env(k, v);
            }
        }
        if let Some(cwd) = &command_cwd {
            command.current_dir(cwd);
        }
        command.stdin(stdin);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        command
            .spawn()
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })
    }

    /// Streaming execution for dyn-sandbox: the sandbox keeps the process alive
    /// and emits `AUTH_REQ:filename:path` on stderr when it blocks an operation.
    /// We read stderr line-by-line, prompt via the attached auth interaction,
    /// and write `ALLOW\n`/`DENY\n` back to stdin (the AUTH control channel).
    ///
    /// The process-side timeout is a deadline that pauses while the user
    /// decides: `interaction.ask` never consumes the timeout.
    async fn exec_linux_dynsandbox(
        &self,
        timeout_ms: Option<u64>,
        mut child: tokio::process::Child,
        stdin: tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
        pgid: i32,
    ) -> Result<ExecResult, OperationError> {
        let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        let mut stdout_task = spawn_pipe_drainer(stdout, stdout_buf.clone());

        // True while stderr_task may still relay AUTH_REQ events. Once it ends
        // the auth channel closes; the main loop stops polling it so a closed
        // channel can't busy-spin past the deadline timer.
        let auth_open = Arc::new(AtomicBool::new(true));
        let (auth_tx, mut auth_rx) = tokio::sync::mpsc::channel::<AuthEvent>(16);
        let mut stderr_task = {
            let buf = stderr_buf.clone();
            let auth_open_flag = auth_open.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut bytes: Vec<u8> = Vec::new();
                loop {
                    bytes.clear();
                    match reader.read_until(b'\n', &mut bytes).await {
                        Ok(0) => break,
                        Err(_) => break,
                        Ok(_) => {}
                    }
                    // Reading raw bytes never fails on encoding; a line that
                    // isn't valid UTF-8 is dropped and reading continues, so a
                    // stray non-UTF-8 byte can't kill the AUTH_REQ relay.
                    let Ok(line) = std::str::from_utf8(&bytes) else {
                        tracing::warn!(
                            "dyn-sandbox dropped non-UTF-8 stderr line ({} raw bytes)",
                            bytes.len()
                        );
                        continue;
                    };
                    if let Some(rest) = line.strip_prefix("AUTH_REQ:") {
                        if let Some((filename, path)) = rest.split_once(':') {
                            let event = AuthEvent {
                                filename: filename.to_string(),
                                path: path.trim().to_string(),
                            };
                            tracing::info!(
                                "dyn-sandbox AUTH_REQ received: filename={} path={}",
                                filename,
                                path
                            );
                            if auth_tx.send(event).await.is_err() {
                                break;
                            }
                        } else {
                            tracing::warn!(
                                "malformed dyn-sandbox AUTH_REQ line: {:?}",
                                line.trim()
                            );
                        }
                    } else {
                        // Regular stderr: buffer for the result.
                        buf.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .extend_from_slice(line.as_bytes());
                    }
                }
                auth_open_flag.store(false, Ordering::SeqCst);
            })
        };

        let mut stdin = stdin;
        let mut deadline =
            timeout_ms.map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let mut exit_code: Option<i32> = None;
        let mut timed_out = false;

        enum Event {
            Exited(std::io::Result<std::process::ExitStatus>),
            Auth { filename: String, path: String },
            StderrClosed,
        }

        loop {
            let wait_for_event = async {
                tokio::select! {
                    status = child.wait() => Event::Exited(status),
                    evt = auth_rx.recv(), if auth_open.load(Ordering::SeqCst) => match evt {
                        Some(auth) => Event::Auth { filename: auth.filename, path: auth.path },
                        None => Event::StderrClosed,
                    }
                }
            };
            let outcome = match deadline {
                Some(dl) => tokio::time::timeout_at(dl, wait_for_event).await,
                None => Ok(wait_for_event.await),
            };

            match outcome {
                Err(_elapsed) => {
                    timed_out = true;
                    tracing::warn!(
                        "dyn-sandbox exec timed out (timeout_ms={:?}), killing process group {}",
                        timeout_ms,
                        pgid
                    );
                    Self::kill_process_group(&mut child, pgid).await;
                    break;
                }
                Ok(Event::Exited(status)) => {
                    exit_code = status.ok().and_then(|status| status.code());
                    break;
                }
                Ok(Event::Auth { filename, path }) => {
                    let ask_start = tokio::time::Instant::now();
                    let decision = self.handle_auth(&filename, &path).await;
                    let ask_elapsed = ask_start.elapsed();
                    tracing::info!(
                        "dyn-sandbox auth decision: filename={} path={} decision={} elapsed_ms={}",
                        filename,
                        path,
                        String::from_utf8_lossy(&decision).trim(),
                        ask_elapsed.as_millis() as u64
                    );
                    if let Some(deadline) = &mut deadline {
                        // Pause the process-side timeout across the user's ask.
                        *deadline += ask_elapsed;
                    }
                    if let Err(error) = stdin.write_all(&decision).await {
                        tracing::error!(
                            "dyn-sandbox failed to write auth decision to stdin: {}",
                            error
                        );
                    }
                    let _ = stdin.flush().await;
                }
                // Only reachable in a tiny race between stderr_task storing
                // `auth_open=false` and dropping `auth_tx`; the channel closing
                // means no more auth events, so just keep waiting on the child.
                Ok(Event::StderrClosed) => {}
            }
        }

        // Bounded grace drain, then abort so we never hang (mirrors the
        // non-streamed path).
        let _ = timeout(DRAIN_GRACE, async {
            let _ = tokio::join!(&mut stdout_task, &mut stderr_task);
        })
        .await;
        stdout_task.abort();
        stderr_task.abort();

        #[cfg(unix)]
        if pgid > 0 {
            crate::process_group::unregister_pgid(pgid);
        }

        let stdout = std::mem::take(&mut *stdout_buf.lock().unwrap_or_else(|e| e.into_inner()));
        let stderr = std::mem::take(&mut *stderr_buf.lock().unwrap_or_else(|e| e.into_inner()));

        tracing::info!(
            "dyn-sandbox exec finished: exit_code={:?} timed_out={} stdout_bytes={} stderr_bytes={}",
            exit_code,
            timed_out,
            stdout.len(),
            stderr.len()
        );

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
            timed_out,
            ..Default::default()
        })
    }

    /// Prompt the user whether to allow a blocked path. Denies when no auth
    /// interaction has been attached (e.g. the backend used standalone).
    async fn handle_auth(&self, filename: &str, path: &str) -> Vec<u8> {
        let interaction = self
            ._state
            .interaction
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        let Some(interaction) = interaction else {
            tracing::warn!(
                "dyn-sandbox auto-deny (no auth interaction attached): filename={} path={}",
                filename,
                path
            );
            return b"DENY\n".to_vec();
        };
        let response = interaction
            .ask(&InteractionRequest::Choice {
                prompt: format!(
                    "Dynamic Sandbox blocked Auth\nTool needs access to:\n file:{filename}\n path:{path}"
                ),
                options: vec!["Allow".to_string(), "Deny".to_string()],
                allow_custom_input: false,
                source: None,
            })
            .await;
        match response {
            InteractionResponse::Choice { value: Some(value) } if value == "Allow" => {
                b"ALLOW\n".to_vec()
            }
            _ => b"DENY\n".to_vec(),
        }
    }

    async fn kill_process_group(child: &mut tokio::process::Child, pgid: i32) {
        #[cfg(unix)]
        {
            if pgid > 0 {
                crate::process_group::send_sigterm_to_group(pgid);
                tokio::time::sleep(Duration::from_millis(300)).await;
                crate::process_group::send_sigkill_to_group(pgid);
            }
        }
        let _ = child.wait().await;
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
    request: &ExecRequest,
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
        if let Some(args) = policy.linux_dynsandbox_args(cwd, request.extra.as_ref()) {
            let mut command = Command::new("dyn-sandbox");
            command.args(args);
            command.arg("--");
            command.arg(spec.program);
            command.args(spec.args);
            tracing::info!(
                "dyn-sandbox command built: {}",
                command
                    .as_std()
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            return command;
        }
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
                ..Default::default()
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

#[cfg(all(test, target_os = "linux"))]
mod linux_dynsandbox_tests {
    use super::*;
    use crate::backends::local::factory::local_backend_with_isolation;
    use crate::backends::local::policy::LocalBackendPolicy;
    use agent_contracts::backend::BackendPath;
    use agent_contracts::InteractionHandle;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    /// Interaction handle that answers every dyn-sandbox AUTH prompt with a
    /// fixed decision, recording the prompts it was shown for assertions.
    struct ScriptedInteraction {
        allow: bool,
        prompts: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl InteractionHandle for ScriptedInteraction {
        async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
            let prompt = match request {
                InteractionRequest::Choice { prompt, .. } => prompt.clone(),
                _ => String::new(),
            };
            self.prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(prompt);
            let value = if self.allow {
                Some("Allow".to_string())
            } else {
                Some("Deny".to_string())
            };
            InteractionResponse::Choice { value }
        }
    }

    #[test]
    fn command_from_spec_builds_linux_dynsandbox_args() {
        let workspace = PathBuf::from("/workspace");
        let policy = LocalBackendPolicy::test_isolated(
            "linux_dynsandbox",
            vec![workspace.clone(), workspace.join("tmp")],
            vec![workspace.join("tmp")],
            false,
        );
        let request = ExecRequest {
            command: "echo hi".to_string(),
            args: vec![],
            shell: Some("bash".to_string()),
            cwd: Some(BackendPath("/workspace".to_string())),
            timeout_ms: Some(1_000),
            ..Default::default()
        };
        let command = command_from_spec(
            &request,
            LocalCommandSpec {
                program: "bash".to_string(),
                args: vec!["-c".to_string(), "echo hi".to_string()],
            },
            &policy,
            Some(workspace.as_path()),
        );

        assert_eq!(command.as_std().get_program(), "dyn-sandbox");
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "--mount",
                "/workspace:ro",
                "--mount",
                "/workspace/tmp:rw",
                "-c",
                "/workspace",
                "--",
                "bash",
                "-c",
                "echo hi",
            ]
        );
    }

    /// Install a fake `dyn-sandbox` script on PATH (once per test process). The
    /// real binary is not installed in this environment, and the streaming
    /// tests need the backend's `linux_dynsandbox_available()` PATH probe to pass.
    /// The script plays the sandbox side of the AUTH protocol: it announces a
    /// blocked path on stderr, reads the decision from stdin (the AUTH control
    /// channel), and echoes it back so tests can observe what was written.
    fn install_fake_linux_dynsandbox() -> &'static PathBuf {
        static FAKE_DYN_SANDBOX: OnceLock<PathBuf> = OnceLock::new();
        FAKE_DYN_SANDBOX.get_or_init(|| {
            let root = std::env::temp_dir().join(format!(
                "xiaoo-fake-dyn-sandbox-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(root.as_path());
            std::fs::create_dir_all(root.as_path()).unwrap();
            let bin = root.join("dyn-sandbox");
            std::fs::write(
                bin.as_path(),
                "#!/bin/sh\necho \"AUTH_REQ:shadow:/etc/shadow\" >&2\nIFS= read -r response\necho \"verdict:$response\"\nexit 0\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(bin.as_path(), std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
            let mut paths =
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
            paths.insert(0, root.clone());
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            root
        })
    }

    fn linux_dynsandbox_backend(
        workspace: PathBuf,
        writable: PathBuf,
    ) -> std::sync::Arc<dyn agent_contracts::backend::OperationBackend> {
        local_backend_with_isolation(
            workspace.clone(),
            None,
            Some(writable.clone()),
            None,
            Some(json!({
                "kind": "linux_dynsandbox",
                "allow_network": false,
                "readable_roots": [workspace.to_string_lossy().to_string()],
                "writable_roots": [writable.to_string_lossy().to_string()]
            })),
        )
        .unwrap()
    }

    async fn linux_dynsandbox_exec_bash(
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
                ..Default::default()
            })
            .await
            .unwrap()
    }

    fn linux_dynsandbox_exec_with_auth(
        workspace: &Path,
        command: &str,
        allow: bool,
    ) -> (ExecResult, Vec<String>) {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        install_fake_linux_dynsandbox();
        let root = super::test_workspace_root("xiaoo-dyn-sandbox-", "auth");
        let writable = workspace.join("tmp");
        std::fs::create_dir_all(writable.as_path()).unwrap();

        let backend = linux_dynsandbox_backend(workspace.to_path_buf(), writable);
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        backend.attach_interaction(Arc::new(ScriptedInteraction {
            allow,
            prompts: prompts.clone(),
        }));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(linux_dynsandbox_exec_bash(
            backend.as_ref(),
            workspace,
            command,
        ));
        let prompts = prompts.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = std::fs::remove_dir_all(root.as_path());
        (result, prompts)
    }

    #[test]
    fn linux_dynsandbox_streaming_allows_blocked_path() {
        let root = super::test_workspace_root("xiaoo-dyn-sandbox-", "allow");
        let workspace = root.join("workspace");
        let (result, prompts) =
            linux_dynsandbox_exec_with_auth(&workspace, "cat /etc/shadow", true);

        assert_eq!(result.exit_code, Some(0));
        assert!(
            String::from_utf8_lossy(result.stdout.as_slice()).contains("verdict:ALLOW"),
            "stdout was: {:?}",
            String::from_utf8_lossy(result.stdout.as_slice())
        );
        assert_eq!(prompts.len(), 1, "prompts: {prompts:?}");
        assert!(
            prompts[0].contains("/etc/shadow"),
            "prompt was: {:?}",
            prompts[0]
        );
        let _ = std::fs::remove_dir_all(root.as_path());
    }

    #[test]
    fn linux_dynsandbox_streaming_denies_on_user_choice() {
        let root = super::test_workspace_root("xiaoo-dyn-sandbox-", "deny");
        let workspace = root.join("workspace");
        let (result, prompts) =
            linux_dynsandbox_exec_with_auth(&workspace, "cat /etc/shadow", false);

        assert_eq!(result.exit_code, Some(0));
        assert!(
            String::from_utf8_lossy(result.stdout.as_slice()).contains("verdict:DENY"),
            "stdout was: {:?}",
            String::from_utf8_lossy(result.stdout.as_slice())
        );
        assert_eq!(prompts.len(), 1);
        let _ = std::fs::remove_dir_all(root.as_path());
    }

    /// Regression test for the UFCS recursion bug in `exec_streaming`:
    /// when `policy.requires_stdin()` is true (LinuxDynsandbox
    /// isolation), the override used to call
    /// `OperationExec::exec_streaming(self, request, sink)` — UFCS that
    /// resolves to `<LocalExec as OperationExec>::exec_streaming` (this
    /// same override), not the trait's default impl. Result: infinite
    /// recursion → stack overflow on every `exec_streaming` call under
    /// dyn-sandbox isolation. The existing streaming tests above use
    /// `exec()` (not `exec_streaming()`), so they didn't cover the
    /// fallback branch.
    ///
    /// With the fix, the override calls `exec_streaming_via_exec`
    /// (free function), which delegates to `exec()` (the AUTH-aware
    /// override) and feeds buffered stdout through the sink.
    #[test]
    fn linux_dynsandbox_exec_streaming_does_not_recurse() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        install_fake_linux_dynsandbox();
        let root = super::test_workspace_root("xiaoo-dyn-sandbox-", "stream-norecurse");
        let workspace = root.join("workspace");
        let writable = workspace.join("tmp");
        std::fs::create_dir_all(writable.as_path()).unwrap();

        let backend = linux_dynsandbox_backend(workspace.to_path_buf(), writable);
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        backend.attach_interaction(Arc::new(ScriptedInteraction {
            allow: true,
            prompts: prompts.clone(),
        }));

        // Sink that collects every line it sees, never asks to stop.
        // Under the bug, `exec_streaming` would recurse before any line
        // reached the sink — so an empty `seen` + a process exit is the
        // first observable symptom of the stack-overflow path.
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        struct CollectAll {
            seen: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl super::LineSink for CollectAll {
            // LineSink::on_line is sync — no async needed.
            fn on_line(&self, line: &str) -> bool {
                self.seen.lock().unwrap().push(line.to_string());
                true
            }
        }
        let sink: Arc<dyn super::LineSink> = Arc::new(CollectAll { seen: seen.clone() });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            // Hard ceiling so a re-introduced recursion doesn't hang
            // the test suite forever — the original bug would spin
            // until the OS killed the process for OOM/stack overflow.
            .build()
            .unwrap();
        let result = runtime
            .block_on(backend.exec().exec_streaming(
                ExecRequest {
                    command: "cat /etc/shadow".to_string(),
                    args: vec![],
                    shell: Some("bash".to_string()),
                    cwd: Some(BackendPath(workspace.to_string_lossy().into_owned())),
                    timeout_ms: Some(5_000),
                    ..Default::default()
                },
                sink,
            ))
            .unwrap();

        // The fake dyn-sandbox script echoes "verdict:ALLOW" on stdout
        // after the AUTH handshake. The sink must have seen it — proves
        // we reached the buffer-then-sink fallback instead of recursing.
        let seen_guard = seen.lock().unwrap();
        assert!(
            seen_guard.iter().any(|line| line.contains("verdict:ALLOW")),
            "sink must have received the verdict line; got {seen_guard:?}"
        );

        // Output ownership contract: `exec_streaming_via_exec` clears
        // stdout unconditionally (the sink owns collected state).
        assert!(
            result.stdout.is_empty(),
            "exec_streaming result.stdout must be empty (sink owns state); got {} bytes",
            result.stdout.len()
        );
        assert!(!result.stopped_early, "sink never returned false");
        assert_eq!(result.exit_code, Some(0));

        // AUTH handshake still fired once — proves the fallback used
        // `exec()` (the AUTH-aware override), not some non-AUTH path.
        let prompts_guard = prompts.lock().unwrap();
        assert_eq!(prompts_guard.len(), 1, "AUTH prompt must fire exactly once");

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    /// Regression test for the auth-channel busy-wait: when stderr_task ends
    /// before the child does (EOF on our read side), the main loop must stop
    /// polling the closed auth channel so the deadline timer arms and the
    /// still-running child is killed on timeout. Without the `auth_open` guard
    /// the loop spins on the closed channel, the timeout never fires, and exec
    /// only returns once the child exits on its own (~5s later) with no
    /// timeout signal.
    #[test]
    fn exec_linux_dynsandbox_times_out_when_stderr_closes_early() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();

        let exec = LocalExec::new(Arc::new(LocalBackendState {
            backend_id: "test".to_string(),
            workspace_root: BackendPath("/workspace".to_string()),
            workspace_root_host: std::env::current_dir().expect("current dir"),
            temp_root_host: std::env::temp_dir(),
            ..Default::default()
        }));

        // Close stderr immediately (EOF on our read side) but keep running well
        // past the 500ms timeout so the deadline is the only thing that can end
        // the exec promptly.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let start = std::time::Instant::now();
        let result = runtime.block_on(async {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg("exec 2>&-; sleep 5")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .expect("spawn child");
            let pgid = child.id().unwrap_or(0) as i32;
            if pgid > 0 {
                crate::process_group::register_pgid(pgid);
            }
            let stdin = child.stdin.take().expect("stdin");
            let stdout = child.stdout.take().expect("stdout");
            let stderr = child.stderr.take().expect("stderr");
            exec.exec_linux_dynsandbox(Some(500), child, stdin, stdout, stderr, pgid)
                .await
                .expect("exec_linux_dynsandbox")
        });
        let elapsed = start.elapsed();

        assert!(
            result.timed_out,
            "expected timeout, got exit_code={:?}",
            result.exit_code
        );
        assert_eq!(result.exit_code, None);
        assert!(
            elapsed < Duration::from_secs(3),
            "exec took {elapsed:?}; without the auth_open guard it spins until `sleep 5` exits"
        );
    }

    /// Regression test for non-UTF-8 stderr: a stray invalid-UTF-8 line must
    /// not kill the stderr reader, or subsequent regular stderr is lost and —
    /// worse — a later `AUTH_REQ` is never relayed (the sandbox would block
    /// waiting for a decision and only fail via timeout). The line is dropped
    /// and reading continues.
    #[test]
    fn exec_linux_dynsandbox_survives_invalid_utf8_stderr_line() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();

        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let exec = LocalExec::new(Arc::new(LocalBackendState {
            backend_id: "test".to_string(),
            workspace_root: BackendPath("/workspace".to_string()),
            workspace_root_host: std::env::current_dir().expect("current dir"),
            temp_root_host: std::env::temp_dir(),
            interaction: std::sync::RwLock::new(Some(Arc::new(ScriptedInteraction {
                allow: true,
                prompts: prompts.clone(),
            }))),
            ..Default::default()
        }));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(async {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(
                    "printf '\\377\\376\\377\\n' >&2; printf 'ok-line\\n' >&2; \
                         echo 'AUTH_REQ:shadow:/etc/shadow' >&2; IFS= read -r resp; \
                         echo \"verdict:$resp\"",
                )
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .expect("spawn child");
            let pgid = child.id().unwrap_or(0) as i32;
            if pgid > 0 {
                crate::process_group::register_pgid(pgid);
            }
            let stdin = child.stdin.take().expect("stdin");
            let stdout = child.stdout.take().expect("stdout");
            let stderr = child.stderr.take().expect("stderr");
            exec.exec_linux_dynsandbox(Some(5_000), child, stdin, stdout, stderr, pgid)
                .await
                .expect("exec_linux_dynsandbox")
        });

        assert_eq!(result.exit_code, Some(0), "stderr={:?}", result.stderr);
        assert_eq!(
            String::from_utf8_lossy(result.stderr.as_slice()),
            "ok-line\n"
        );
        assert!(
            String::from_utf8_lossy(result.stdout.as_slice()).contains("verdict:ALLOW"),
            "stdout was: {:?}",
            String::from_utf8_lossy(result.stdout.as_slice())
        );
        let prompts = prompts.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(prompts.len(), 1, "prompts: {prompts:?}");
        assert!(
            prompts[0].contains("/etc/shadow"),
            "prompt was: {:?}",
            prompts[0]
        );
    }
}

#[cfg(all(test, unix))]
mod pipe_drain_tests {
    use crate::backends::local::factory::local_backend;
    use agent_contracts::backend::{capability::exec::ExecRequest, BackendPath};
    use std::sync::{Arc, Mutex};
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
                    ..Default::default()
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
                    ..Default::default()
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

    // Streaming exec + line sink that returns `false` after N lines must
    // (a) kill the child process group, (b) set `stopped_early = true`,
    // (c) not buffer the remaining N+1..∞ lines into memory. This is the
    // upstream throttle that bounds runaway `rg`/`grep` output before it
    // ever reaches the downstream truncation layer in `agent_loop.rs`.
    //
    // We use `seq 1 1_000_000` as the producer (one million lines, takes
    // seconds to fully emit) and a sink that stops after 5 lines. The
    // streaming exec must return well before the producer finishes
    // (otherwise this test would take seconds).
    struct FiveLineCollector {
        kept: Mutex<Vec<String>>,
    }
    impl super::LineSink for FiveLineCollector {
        fn on_line(&self, line: &str) -> bool {
            let mut guard = self.kept.lock().unwrap();
            if guard.len() >= 5 {
                return false;
            }
            guard.push(line.to_string());
            true
        }
    }

    #[test]
    fn exec_streaming_kills_child_when_sink_returns_false() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = super::test_workspace_root("xiaoo-stream-", "kill");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let collector = Arc::new(FiveLineCollector {
            kept: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn super::LineSink> = Arc::clone(&collector) as Arc<dyn super::LineSink>;

        let start = Instant::now();
        let result = runtime.block_on(async {
            backend
                .exec()
                .exec_streaming(
                    ExecRequest {
                        // `seq 1 1000000` emits a million lines over a few
                        // seconds. If streaming + early kill works, we return
                        // immediately after 5 lines.
                        command: "seq 1 1000000".to_string(),
                        args: vec![],
                        shell: Some("bash".to_string()),
                        cwd: Some(BackendPath(workspace.to_string_lossy().into_owned())),
                        timeout_ms: Some(30_000), // generous upper bound; we expect to return way before
                        ..Default::default()
                    },
                    sink,
                )
                .await
                .unwrap()
        });
        let elapsed = start.elapsed();

        // Sink asked for early termination — flag must be set.
        assert!(
            result.stopped_early,
            "stopped_early must be true when the sink returned false; got {result:?}"
        );

        // We must NOT have hit the overall timeout — that's the whole
        // point of streaming + early kill.
        assert!(
            !result.timed_out,
            "must not time out; expected to kill child after 5 lines"
        );

        // Bounded wall-clock: a million lines take seconds to emit
        // fully. The early-kill path should return in well under a
        // second (5 lines + process-group teardown ~300ms grace).
        assert!(
            elapsed < Duration::from_secs(5),
            "exec_streaming took {elapsed:?}, expected to return within seconds after early kill"
        );

        // Sink collected exactly 5 lines (the cap), no more — i.e. the
        // remaining 999,995 lines were never read into memory.
        let guard = collector.kept.lock().unwrap();
        assert_eq!(
            guard.len(),
            5,
            "collector must hold exactly 5 lines, got {}",
            guard.len()
        );
        assert_eq!(guard[0], "1");
        assert_eq!(guard[4], "5");

        // stdout in the result is empty — the sink owns collected state,
        // streaming bypassed the buffering.
        assert!(
            result.stdout.is_empty(),
            "streaming result.stdout must be empty (sink owns state); got {} bytes",
            result.stdout.len()
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    // Streaming exec with a sink that never returns `false` (consumer
    // wants everything) must behave like the non-streaming `exec` —
    // exit code is the child's natural exit, `stopped_early = false`,
    // and the sink sees every line in order.
    struct AllLinesCollector {
        kept: Mutex<Vec<String>>,
    }
    impl super::LineSink for AllLinesCollector {
        fn on_line(&self, line: &str) -> bool {
            self.kept.lock().unwrap().push(line.to_string());
            true
        }
    }

    #[test]
    fn exec_streaming_drains_to_eof_when_sink_never_stops() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = super::test_workspace_root("xiaoo-stream-", "eof");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let collector = Arc::new(AllLinesCollector {
            kept: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn super::LineSink> = Arc::clone(&collector) as Arc<dyn super::LineSink>;

        let result = runtime.block_on(async {
            backend
                .exec()
                .exec_streaming(
                    ExecRequest {
                        command: "printf 'a\\nb\\nc\\n'".to_string(),
                        args: vec![],
                        shell: Some("bash".to_string()),
                        cwd: Some(BackendPath(workspace.to_string_lossy().into_owned())),
                        timeout_ms: Some(5_000),
                        ..Default::default()
                    },
                    sink,
                )
                .await
                .unwrap()
        });

        // Child exited naturally → exit_code 0, no early kill.
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.stopped_early);
        assert!(!result.timed_out);

        // Sink observed all 3 lines in order.
        let guard = collector.kept.lock().unwrap();
        assert_eq!(*guard, vec!["a", "b", "c"]);

        let _ = std::fs::remove_dir_all(root.as_path());
    }
}
