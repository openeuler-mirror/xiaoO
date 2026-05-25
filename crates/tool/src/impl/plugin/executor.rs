use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::manifest::{LoadedDeclarativeTool, StdinMode, StdoutMode};
use super::spec::DeclarativeToolSpec;

pub struct DeclarativeToolExecutor {
    spec: Arc<DeclarativeToolSpec>,
    manifest_path: PathBuf,
    tool_dir: PathBuf,
    command: String,
    args: Vec<String>,
    timeout_ms: u64,
    stdin_mode: StdinMode,
    stdout_mode: StdoutMode,
    env_names: Vec<String>,
}

impl DeclarativeToolExecutor {
    pub fn from_loaded_tool(spec: Arc<DeclarativeToolSpec>, tool: &LoadedDeclarativeTool) -> Self {
        Self {
            spec,
            manifest_path: tool.manifest_path.clone(),
            tool_dir: tool.tool_dir.clone(),
            command: tool.manifest.exec.command.clone(),
            args: tool.manifest.exec.args.clone(),
            timeout_ms: tool.manifest.timeout_ms,
            stdin_mode: tool.manifest.exec.stdin,
            stdout_mode: tool.manifest.exec.stdout,
            env_names: tool.manifest.exec.env.clone(),
        }
    }

    fn stdin_payload(&self, call: &FinalToolCall, runtime: &dyn RuntimeView) -> serde_json::Value {
        let workspace_root = runtime.agent_context().workspace().root.clone();
        let metadata = runtime.agent_context().metadata();
        json!({
            "args": call.input,
            "context": {
                "agent_id": metadata.agent_id,
                "model": metadata.model,
                "session_id": metadata.session_id,
                "directory": workspace_root,
                "worktree": workspace_root,
                "tool_dir": self.tool_dir,
            }
        })
    }

    async fn invoke_process(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<RawToolOutcome, ToolExecutionError> {
        let workspace_root = runtime.agent_context().workspace().root.clone();
        let mut command = Command::new(&self.command);
        command
            .args(&self.args)
            .kill_on_drop(true)
            .current_dir(&workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("XIAOO_WORKSPACE_ROOT", &workspace_root)
            .env("XIAOO_TOOL_MANIFEST", &self.manifest_path)
            .env("XIAOO_TOOL_DIR", &self.tool_dir);

        if let Some(session_id) = runtime.agent_context().metadata().session_id.as_deref() {
            command.env("XIAOO_SESSION_ID", session_id);
        }
        command.env(
            "XIAOO_AGENT_ID",
            &runtime.agent_context().metadata().agent_id,
        );

        for env_name in &self.env_names {
            if let Ok(value) = std::env::var(env_name) {
                command.env(env_name, value);
            }
        }

        match self.stdin_mode {
            StdinMode::Json => {
                command.stdin(Stdio::piped());
            }
            StdinMode::None => {
                command.stdin(Stdio::null());
            }
        }

        let mut child = command
            .spawn()
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to spawn custom tool '{}': {error}",
                    self.spec.name().0
                ),
            })?;

        if self.stdin_mode == StdinMode::Json {
            let payload =
                serde_json::to_vec(&self.stdin_payload(call, runtime)).map_err(|error| {
                    ToolExecutionError::ExecutionFailed {
                        message: format!("failed to serialize custom tool input: {error}"),
                    }
                })?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(&payload).await.map_err(|error| {
                    ToolExecutionError::ExecutionFailed {
                        message: format!("failed to write custom tool stdin: {error}"),
                    }
                })?;
            }
        }

        let output = timeout(
            Duration::from_millis(self.timeout_ms),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| ToolExecutionError::Timeout {
            timeout_ms: self.timeout_ms,
        })?
        .map_err(|error| ToolExecutionError::ExecutionFailed {
            message: format!(
                "failed to wait for custom tool '{}': {error}",
                self.spec.name().0
            ),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            return Ok(RawToolOutcome::Error {
                message: format!(
                    "custom tool '{}' exited with status {}{}",
                    self.spec.name().0,
                    output.status,
                    if stderr.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", stderr.trim())
                    }
                ),
            });
        }

        match self.stdout_mode {
            StdoutMode::Text => Ok(RawToolOutcome::Success { output: stdout }),
            StdoutMode::Json => {
                let value: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
                    ToolExecutionError::ExecutionFailed {
                        message: format!(
                            "custom tool '{}' returned invalid JSON: {error}",
                            self.spec.name().0
                        ),
                    }
                })?;
                Ok(RawToolOutcome::Success {
                    output: value.to_string(),
                })
            }
        }
    }
}

#[async_trait]
impl ToolExecutor for DeclarativeToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        Ok(ToolExecutorOutput::Completed {
            raw_outcome: self.invoke_process(call, runtime).await?,
        })
    }
}
