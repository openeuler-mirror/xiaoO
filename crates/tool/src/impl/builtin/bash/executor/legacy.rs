use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::r#impl::path_resolver::{expand_path_from_base, runtime_workspace_root};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::constants::default_timeout_ms;
use super::input::BashInput;
use super::output::BashOutput;
use super::spec::BashToolSpec;
use super::validation;

const SHELL_COMMAND_FLAG: &str = "-c";
const MAX_OUTPUT_BYTES_PER_STREAM: usize = 1024 * 1024;

pub struct BashExecutor {
    spec: Arc<BashToolSpec>,
}

struct CollectedPipeOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl BashExecutor {
    pub fn new(spec: Arc<BashToolSpec>) -> Self {
        Self { spec }
    }

    async fn read_pipe<R>(reader: Option<R>) -> Result<CollectedPipeOutput, String>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let Some(mut reader) = reader else {
            return Ok(CollectedPipeOutput {
                bytes: Vec::new(),
                truncated: false,
            });
        };

        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 8192];
        let mut truncated = false;

        loop {
            let read = reader
                .read(&mut chunk)
                .await
                .map_err(|e| format!("Failed to read process output: {}", e))?;
            if read == 0 {
                break;
            }

            let available = MAX_OUTPUT_BYTES_PER_STREAM.saturating_sub(buffer.len());
            let copy_len = available.min(read);
            if copy_len > 0 {
                buffer.extend_from_slice(&chunk[..copy_len]);
            }
            if copy_len < read {
                truncated = true;
            }
        }

        Ok(CollectedPipeOutput {
            bytes: buffer,
            truncated,
        })
    }

    fn append_shell_command(command: &mut Command, shell_command: &str) {
        command.arg(SHELL_COMMAND_FLAG).arg(shell_command);
    }

    async fn execute_with_shell(
        shell: &str,
        input: &BashInput,
        base_dir: &std::path::Path,
    ) -> Result<BashOutput, String> {
        let timeout_ms = input.timeout.unwrap_or_else(default_timeout_ms);

        let mut command = Command::new(shell);
        Self::append_shell_command(&mut command, &input.command);
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let resolved_cwd = input
            .cwd
            .as_deref()
            .map(|cwd| expand_path_from_base(cwd, base_dir))
            .unwrap_or_else(|| base_dir.to_string_lossy().into_owned());
        command.current_dir(resolved_cwd);

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

        let stdout = stdout_task
            .await
            .map_err(|e| format!("Failed to join stdout reader task: {}", e))??;
        let stderr = stderr_task
            .await
            .map_err(|e| format!("Failed to join stderr reader task: {}", e))??;

        let output = BashOutput {
            stdout: String::from_utf8_lossy(&stdout.bytes).replace("\r\n", "\n"),
            stdout_truncated: stdout.truncated,
            stderr: String::from_utf8_lossy(&stderr.bytes).replace("\r\n", "\n"),
            stderr_truncated: stderr.truncated,
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
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: BashInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let workspace_root = runtime_workspace_root(runtime);
        let validation_result = validation::validate_input_with_base(&input, workspace_root);
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

        let output = Self::execute_with_shell("bash", &input, workspace_root)
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

#[cfg(test)]
mod tests {
    use super::{BashExecutor, BashInput, MAX_OUTPUT_BYTES_PER_STREAM};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tokio::io::AsyncReadExt;
    use uuid::Uuid;

    fn make_temp_test_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("xiaoo-bash-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).expect("temp test dir should be created");
        path
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_pipe_truncates_without_growing_unbounded() {
        let reader = tokio::io::repeat(b'a').take((MAX_OUTPUT_BYTES_PER_STREAM + 512) as u64);

        let output = BashExecutor::read_pipe(Some(reader))
            .await
            .expect("pipe should read");

        assert_eq!(output.bytes.len(), MAX_OUTPUT_BYTES_PER_STREAM);
        assert!(output.truncated);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_with_shell_uses_non_login_flag() {
        let temp_dir = make_temp_test_dir();
        let shell_path = temp_dir.join("fake-shell.sh");
        let flag_path = temp_dir.join("flag.txt");
        fs::write(
            &shell_path,
            format!(
                "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\nexit 0\n",
                flag_path.display()
            ),
        )
        .expect("shell script should write");

        let mut permissions = fs::metadata(&shell_path)
            .expect("shell script should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&shell_path, permissions).expect("shell script should be executable");

        let output = BashExecutor::execute_with_shell(
            shell_path.to_str().expect("path should be utf-8"),
            &BashInput {
                command: "echo ignored".to_string(),
                cwd: None,
                timeout: Some(1_000),
            },
            &temp_dir,
        )
        .await
        .expect("executor should run fake shell");

        assert_eq!(output.exit_code, Some(0));
        let flag = fs::read_to_string(flag_path).expect("flag should be captured");
        assert_eq!(flag, "-c");

        fs::remove_dir_all(temp_dir).expect("temp dir should be removed");
    }
}
