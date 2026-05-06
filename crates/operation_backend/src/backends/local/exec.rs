use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;
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
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        let mut command = build_command(self._state.default_shell.as_deref(), &request)?;

        if let Some(env_vars) = &request.env {
            for (k, v) in env_vars {
                command.env(k, v);
            }
        }

        if let Some(cwd) = request.cwd.as_ref() {
            let cwd = self._state.backend_path_to_host(cwd)?;
            self._state.ensure_directory(cwd.as_path())?;
            command.current_dir(cwd);
        }

        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "failed to capture stdout".to_string(),
            })?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "failed to capture stderr".to_string(),
            })?;

        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });

        let (exit_code, timed_out) = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
                Ok(status) => {
                    let status = status.map_err(|error| OperationError::ExecutionFailed {
                        message: error.to_string(),
                    })?;
                    (status.code(), false)
                }
                Err(_) => {
                    child
                        .kill()
                        .await
                        .map_err(|error| OperationError::ExecutionFailed {
                            message: error.to_string(),
                        })?;
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
            (status.code(), false)
        };

        let stdout = stdout_task
            .await
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;
        let stderr = stderr_task
            .await
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
            timed_out,
        })
    }
}

fn build_command(
    default_shell: Option<&str>,
    request: &ExecRequest,
) -> Result<Command, OperationError> {
    if request.command.trim().is_empty() {
        return Err(OperationError::ExecutionFailed {
            message: "command cannot be empty".to_string(),
        });
    }

    if let Some(shell) = request.shell.as_deref().or(default_shell) {
        if !request.args.is_empty() {
            return Err(OperationError::Unsupported {
                message: "shell execution does not support args".to_string(),
            });
        }
        let mut command = Command::new(shell);
        command.arg("-c").arg(request.command.as_str());
        return Ok(command);
    }

    let mut command = Command::new(request.command.as_str());
    command.args(request.args.iter());
    Ok(command)
}
