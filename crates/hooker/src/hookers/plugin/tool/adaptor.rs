use std::any::Any;
use std::io::Write;
use std::process::{Command, Stdio};

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hooker::HookPointId;
use agent_types::hooker::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput, RawToolOutcome, ToolExecutionError,
};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{resolve_hook_point_category, HookPointCategory};

pub(crate) struct PluginToolHookerAdaptor {
    id: HookerId,
    hook_point: HookPointId,
    command: String,
    definition: serde_json::Value,
}

impl PluginToolHookerAdaptor {
    pub fn new(
        id: HookerId,
        hook_point: HookPointId,
        command: String,
        definition: serde_json::Value,
    ) -> Self {
        Self {
            id,
            hook_point,
            command,
            definition,
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn definition(&self) -> &serde_json::Value {
        &self.definition
    }

    async fn invoke_for_category(
        &self,
        category: HookPointCategory,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        match (category, input) {
            (HookPointCategory::ToolPre, HookInvokeInput::Pre(input)) => {
                self.invoke_pre(&input, runtime).await
            }
            (HookPointCategory::ToolPost, HookInvokeInput::Post(input)) => {
                self.invoke_post(&input, runtime).await
            }
            (HookPointCategory::ToolError, HookInvokeInput::Error(input)) => {
                self.invoke_error(&input, runtime).await
            }
            (category, _) => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' received mismatched invoke input for category {:?}",
                    self.id.0, category
                ),
            }),
        }
    }

    async fn invoke_pre(
        &self,
        input: &PreToolHookInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_pre_payload(input, runtime)?;
        let output = self.run_plugin_command(&payload)?;
        Ok(HookInvokeOutput::Pre(self.parse_pre_result(&output)?))
    }

    async fn invoke_post(
        &self,
        input: &PostToolHookInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_post_payload(input, runtime)?;
        let output = self.run_plugin_command(&payload)?;
        Ok(HookInvokeOutput::Post(self.parse_post_result(&output)?))
    }

    async fn invoke_error(
        &self,
        input: &ErrorToolHookInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_error_payload(input, runtime)?;
        let output = self.run_plugin_command(&payload)?;
        Ok(HookInvokeOutput::Error(self.parse_error_result(&output)?))
    }

    fn build_pre_payload(
        &self,
        input: &PreToolHookInput,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "pre",
            "hooker": self.serialize_hooker_info(runtime),
            "call": serde_json::to_value(&input.call).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize pre-hook call payload for '{}': {}", self.id.0, error),
            })?,
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn build_post_payload(
        &self,
        input: &PostToolHookInput,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "post",
            "hooker": self.serialize_hooker_info(runtime),
            "call": serde_json::to_value(&input.call).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize post-hook call payload for '{}': {}", self.id.0, error),
            })?,
            "outcome": self.serialize_raw_outcome(&input.outcome),
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn build_error_payload(
        &self,
        input: &ErrorToolHookInput,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "error",
            "hooker": self.serialize_hooker_info(runtime),
            "call": serde_json::to_value(&input.call).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize error-hook call payload for '{}': {}", self.id.0, error),
            })?,
            "error": self.serialize_execution_error(&input.error),
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn serialize_hooker_info(&self, runtime: &dyn RuntimeView) -> Value {
        json!({
            "id": self.id.0,
            "hook_point": self.hook_point.0,
            "command": self.command,
            "agent_id": runtime.agent_context().metadata().agent_id,
        })
    }

    fn serialize_raw_outcome(&self, outcome: &RawToolOutcome) -> Value {
        match outcome {
            RawToolOutcome::Success { output } => json!({
                "type": "success",
                "output": output,
            }),
            RawToolOutcome::Error { message } => json!({
                "type": "error",
                "message": message,
            }),
        }
    }

    fn serialize_execution_error(&self, error: &ToolExecutionError) -> Value {
        match error {
            ToolExecutionError::NotFound { tool_name } => json!({
                "type": "not_found",
                "tool_name": tool_name,
                "message": error.to_string(),
            }),
            ToolExecutionError::ExecutionFailed { message } => json!({
                "type": "execution_failed",
                "message": message,
            }),
            ToolExecutionError::Timeout { timeout_ms } => json!({
                "type": "timeout",
                "timeout_ms": timeout_ms,
                "message": error.to_string(),
            }),
            ToolExecutionError::PermissionDenied { message } => json!({
                "type": "permission_denied",
                "message": message,
            }),
        }
    }

    fn run_plugin_command(&self, payload: &Value) -> Result<Value, ToolExecutionError> {
        let payload_bytes =
            serde_json::to_vec(payload).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to serialize plugin command payload for hooker '{}': {}",
                    self.id.0, error
                ),
            })?;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to spawn plugin command for hooker '{}' (command='{}'): {}",
                    self.id.0, self.command, error
                ),
            })?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(&payload_bytes).map_err(|error| {
                ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "failed to write stdin for plugin hooker '{}' (command='{}'): {}",
                        self.id.0, self.command, error
                    ),
                }
            })?;
        }

        let output =
            child
                .wait_with_output()
                .map_err(|error| ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "failed to wait for plugin hooker '{}' (command='{}'): {}",
                        self.id.0, self.command, error
                    ),
                })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' command '{}' exited with status {}{}",
                    self.id.0,
                    self.command,
                    output.status,
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", stderr)
                    }
                ),
            });
        }

        serde_json::from_slice(&output.stdout).map_err(|error| {
            ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' command '{}' returned invalid JSON: {}",
                    self.id.0, self.command, error
                ),
            }
        })
    }

    fn parse_pre_result(&self, output: &Value) -> Result<PreHookResult, ToolExecutionError> {
        match self.read_required_result_tag(output)?.as_str() {
            "allow" => Ok(PreHookResult::Allow),
            "deny" => Ok(PreHookResult::Deny {
                reason: self
                    .read_required_string_field(output, "reason")?
                    .to_string(),
            }),
            "transform" => Ok(PreHookResult::Transform {
                modified_input: self
                    .read_required_value_field(output, "modified_input")?
                    .clone(),
            }),
            result => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin tool pre-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_post_result(&self, output: &Value) -> Result<PostHookResult, ToolExecutionError> {
        match self.read_required_result_tag(output)?.as_str() {
            "accept" => Ok(PostHookResult::Accept),
            "transform" => Ok(PostHookResult::Transform {
                modified_output: self
                    .read_required_string_field(output, "modified_output")?
                    .to_string(),
            }),
            result => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin tool post-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_error_result(&self, output: &Value) -> Result<ErrorHookResult, ToolExecutionError> {
        match self.read_required_result_tag(output)?.as_str() {
            "propagate" => Ok(ErrorHookResult::Propagate),
            "recover" => Ok(ErrorHookResult::Recover {
                output: self
                    .read_required_string_field(output, "output")?
                    .to_string(),
            }),
            result => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin tool error-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn read_required_result_tag(&self, output: &Value) -> Result<String, ToolExecutionError> {
        Ok(self
            .read_required_string_field(output, "result")?
            .to_lowercase())
    }

    fn read_required_string_field<'a>(
        &self,
        output: &'a Value,
        field_name: &str,
    ) -> Result<&'a str, ToolExecutionError> {
        output
            .get(field_name)
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' response must contain string field '{}'",
                    self.id.0, field_name
                ),
            })
    }

    fn read_required_value_field<'a>(
        &self,
        output: &'a Value,
        field_name: &str,
    ) -> Result<&'a Value, ToolExecutionError> {
        output
            .get(field_name)
            .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' response must contain field '{}'",
                    self.id.0, field_name
                ),
            })
    }
}

#[async_trait]
impl Hooker for PluginToolHookerAdaptor {
    fn id(&self) -> &HookerId {
        &self.id
    }

    fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let category = resolve_hook_point_category(&self.hook_point).map_err(|error| {
            HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to resolve hook point category for hooker '{}': {}",
                    self.id.0, error
                ),
            })
        })?;

        self.invoke_for_category(category, input, runtime)
            .await
            .map_err(HookInvokeError::from)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
