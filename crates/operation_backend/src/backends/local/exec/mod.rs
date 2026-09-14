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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

mod command_spec;
mod pipe_drain;

use self::command_spec::{build_command_spec, command_from_spec};
use self::pipe_drain::{spawn_pipe_drainer, DRAIN_GRACE};

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
