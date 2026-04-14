use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::constants::default_timeout_ms;
use super::input::BashInput;
use super::output::BashOutput;
use super::spec::BashToolSpec;
use super::validation;

pub struct BashExecutor {
    spec: Arc<BashToolSpec>,
}

impl BashExecutor {
    pub fn new(spec: Arc<BashToolSpec>) -> Self {
        Self { spec }
    }

    async fn read_pipe<R>(reader: Option<R>) -> Result<Vec<u8>, String>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let Some(mut reader) = reader else {
            return Ok(Vec::new());
        };

        let mut buffer = Vec::new();
        reader
            .read_to_end(&mut buffer)
            .await
            .map_err(|e| format!("Failed to read process output: {}", e))?;
        Ok(buffer)
    }

    async fn execute_with_shell(shell: &str, input: &BashInput) -> Result<BashOutput, String> {
        let timeout_ms = input.timeout.unwrap_or_else(default_timeout_ms);

        let mut command = Command::new(shell);
        command
            .arg("-lc")
            .arg(&input.command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(cwd) = input.cwd.as_deref() {
            command.current_dir(cwd);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to spawn bash process: {}", e))?;

        let stdout_reader = child.stdout.take();
        let stderr_reader = child.stderr.take();

        let stdout_task = tokio::spawn(Self::read_pipe(stdout_reader));
        let stderr_task = tokio::spawn(Self::read_pipe(stderr_reader));

        let (exit_code, interrupted) =
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
                Ok(wait_result) => {
                    let status = wait_result
                        .map_err(|e| format!("Failed to wait for bash process: {}", e))?;
                    (status.code(), false)
                }
                Err(_) => {
                    child.kill().await.map_err(|e| {
                        format!("Failed to terminate timed out bash process: {}", e)
                    })?;
                    let status = child
                        .wait()
                        .await
                        .map_err(|e| format!("Failed to reap timed out bash process: {}", e))?;
                    (status.code(), true)
                }
            };

        let stdout_bytes = stdout_task
            .await
            .map_err(|e| format!("Failed to join stdout reader task: {}", e))??;
        let stderr_bytes = stderr_task
            .await
            .map_err(|e| format!("Failed to join stderr reader task: {}", e))??;

        let output = BashOutput {
            stdout: String::from_utf8_lossy(&stdout_bytes).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&stderr_bytes).replace("\r\n", "\n"),
            exit_code,
            interrupted,
        };

        Ok(output)
    }
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
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: BashInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let validation_result = validation::validate_input(&input);
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

        let output = Self::execute_with_shell("bash", &input)
            .await
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        let serialized =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output: serialized },
        })
    }
}
