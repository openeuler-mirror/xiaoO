use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
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

    fn expand_tilde_path(value: &str) -> Option<PathBuf> {
        if value == "~" {
            return std::env::var_os("HOME").map(PathBuf::from);
        }

        value.strip_prefix("~/").and_then(|suffix| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix))
        })
    }

    fn resolve_command_token(&self, value: &str) -> String {
        if let Some(expanded) = Self::expand_tilde_path(value) {
            return expanded.to_string_lossy().into_owned();
        }

        if value.starts_with("./") || value.starts_with("../") {
            return self
                .tool_dir
                .join(Path::new(value))
                .to_string_lossy()
                .into_owned();
        }

        value.to_string()
    }

    fn resolve_arg_token(&self, value: &str) -> String {
        if let Some(expanded) = Self::expand_tilde_path(value) {
            return expanded.to_string_lossy().into_owned();
        }

        if value.starts_with("./") || value.starts_with("../") {
            return self.tool_dir.join(value).to_string_lossy().into_owned();
        }

        value.to_string()
    }

    async fn invoke_process(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<RawToolOutcome, ToolExecutionError> {
        let workspace_root = runtime.agent_context().workspace().root.clone();
        let command_token = self.resolve_command_token(&self.command);
        let args = self
            .args
            .iter()
            .map(|arg| self.resolve_arg_token(arg))
            .collect::<Vec<_>>();
        let mut command = Command::new(command_token);
        command
            .args(&args)
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

#[cfg(test)]
mod tests {
    use super::super::manifest::{DeclarativeToolManifest, EffectSection, ExecSection};
    use super::*;

    fn test_executor(tool_dir: PathBuf) -> DeclarativeToolExecutor {
        let loaded = LoadedDeclarativeTool {
            manifest_path: tool_dir.join("test_tool.toml"),
            tool_dir,
            manifest: DeclarativeToolManifest {
                name: "test_tool".to_string(),
                description: "test tool".to_string(),
                timeout_ms: 5000,
                output: None,
                effect: EffectSection::default(),
                input_schema: toml::Value::Table(toml::map::Map::new()),
                exec: ExecSection {
                    command: "sh".to_string(),
                    args: Vec::new(),
                    stdin: StdinMode::Json,
                    stdout: StdoutMode::Text,
                    env: Vec::new(),
                },
            },
            input_schema_json: json!({}),
        };
        let spec = Arc::new(DeclarativeToolSpec::from_loaded_tool(&loaded));
        DeclarativeToolExecutor::from_loaded_tool(spec, &loaded)
    }

    #[test]
    fn expands_tilde_for_explicit_tool_paths() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let executor = test_executor(PathBuf::from("/workspace/.xiaoo/tools"));
        let resolved = executor.resolve_arg_token("~/.xiaoo/tools/md_to_html.mjs");
        assert_eq!(
            resolved,
            PathBuf::from(home)
                .join(".xiaoo/tools/md_to_html.mjs")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn resolves_dot_relative_tool_paths_from_manifest_dir() {
        let executor = test_executor(PathBuf::from("/home/user/.xiaoo/tools"));

        assert_eq!(
            executor.resolve_arg_token("./md_to_html.mjs"),
            PathBuf::from("/home/user/.xiaoo/tools")
                .join("./md_to_html.mjs")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(
            executor.resolve_command_token("./runner.sh"),
            PathBuf::from("/home/user/.xiaoo/tools")
                .join("./runner.sh")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn keeps_workspace_relative_args_unchanged() {
        let executor = test_executor(PathBuf::from("/home/user/.xiaoo/tools"));

        assert_eq!(
            executor.resolve_arg_token(".xiaoo/tools/echo_payload.sh"),
            ".xiaoo/tools/echo_payload.sh"
        );
    }
}
