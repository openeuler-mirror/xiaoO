use std::any::Any;
use std::process::Stdio;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::interaction::types::InteractionSource;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::llm::MessageRole;
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput, RawToolOutcome, ToolExecutionError,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::{resolve_hook_point_category, HookPointCategory};

/// plugin hooker 子进程最长执行时间(10 分钟)。超时后由 `kill_on_drop` 自动兜底杀掉子进程,
/// 防止卡死命令长期阻塞 tokio worker。
const PLUGIN_HOOKER_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) struct PluginToolHookerAdaptor {
    id: HookerId,
    hook_point: HookPointId,
    command: String,
    definition: serde_json::Value,
}

#[derive(Debug)]
enum PluginCommandResponse {
    Final(Value),
    AskUser(AskUserDirective),
}

#[derive(Debug)]
struct AskUserDirective {
    request: PluginAskUserRequest,
    continuation: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PluginAskUserRequest {
    Confirm {
        prompt: String,
    },
    TextInput {
        prompt: String,
    },
    Choice {
        prompt: String,
        options: Vec<String>,
        allow_custom_input: bool,
    },
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

    async fn invoke_for_category(
        &self,
        category: HookPointCategory,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        match (category, input) {
            (HookPointCategory::ToolPre, HookInvokeInput::Pre { input, metadata }) => {
                self.invoke_pre(&input, &metadata, runtime).await
            }
            (HookPointCategory::ToolPost, HookInvokeInput::Post { input, metadata }) => {
                self.invoke_post(&input, &metadata, runtime).await
            }
            (HookPointCategory::ToolError, HookInvokeInput::Error { input, metadata }) => {
                self.invoke_error(&input, &metadata, runtime).await
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
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_pre_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::Pre(self.parse_pre_result(&output)?))
    }

    async fn invoke_post(
        &self,
        input: &PostToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_post_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::Post(self.parse_post_result(&output)?))
    }

    async fn invoke_error(
        &self,
        input: &ErrorToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_error_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::Error(self.parse_error_result(&output)?))
    }

    async fn resolve_plugin_output(
        &self,
        initial_payload: Value,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        let mut payload = initial_payload;

        loop {
            let output = self.run_plugin_command(&payload).await?;
            match self.parse_plugin_command_response(output)? {
                PluginCommandResponse::Final(final_output) => return Ok(final_output),
                PluginCommandResponse::AskUser(directive) => {
                    let request = self.with_hooker_interaction_source(directive.request);
                    let response = runtime.interaction().ask(&request).await;
                    payload = self.build_interaction_followup_payload(
                        payload,
                        directive.continuation,
                        &request,
                        &response,
                    )?;
                }
            }
        }
    }

    fn build_pre_payload(
        &self,
        input: &PreToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        // Get session_id (for cache key)
        let session_id = runtime
            .agent_context()
            .metadata()
            .session_id
            .clone()
            .unwrap_or_else(|| input.call.call_id.clone());

        let recent_messages = runtime.agent_context().conversation().recent_messages(100);

        // Get the most recent user message as prompt_session (for intent consistency check).
        // intent 一致性检查需要"当前意图"，而非整个会话的第一条用户消息：多轮对话里
        // 会话开头通常是寒暄（如 "hi"），取第一条会污染意图判断。用户对 ask_user_question
        // 之类的回答会作为 tool_result（Tool 角色）写回，不会成为 User 消息，因此取
        // 最近一条 User 消息不会被这类回答污染，语义上就是"用户最近一次主动输入"。
        let prompt_session = recent_messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
            .and_then(|m| {
                m.blocks.iter().find_map(|b| match b {
                    agent_types::llm::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();

        // Get completed tool call history (for read_before_write rules)
        // Collect ToolUse (with input params like file paths) and ToolResult (with execution results)
        let messages = recent_messages;
        let mut tool_use_map: std::collections::HashMap<&String, Value> =
            std::collections::HashMap::new();

        // First collect all ToolUse, record call_id -> input mapping
        for m in messages.iter() {
            for block in &m.blocks {
                if let agent_types::llm::ContentBlock::ToolUse {
                    call_id,
                    tool_name,
                    input,
                } = block
                {
                    tool_use_map.insert(
                        call_id,
                        json!({
                            "action_type": tool_name,
                            "action_detail": input,
                        }),
                    );
                }
            }
        }

        // Then collect ToolResult, merge input and output
        let action_history: Vec<Value> = messages
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|block| match block {
                agent_types::llm::ContentBlock::ToolResult {
                    call_id,
                    tool_name,
                    output,
                    is_error,
                } => {
                    // Merge ToolUse input info
                    let mut entry = tool_use_map.get(call_id).cloned().unwrap_or_else(|| {
                        json!({
                            "action_type": tool_name,
                            "action_detail": "",
                        })
                    });
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("call_id".to_string(), json!(call_id));
                        obj.insert("output".to_string(), json!(output));
                        obj.insert("is_error".to_string(), json!(is_error));
                    }
                    Some(entry)
                }
                _ => None,
            })
            .collect();

        // 构造交错历史 prompt（prompt_history）：按时间序遍历消息，遇到 User 消息
        // 开一个新 turn，其后的 ToolResult 归入该 turn 的 actions，直到下一条 User。
        // 解决多轮会话里"继续"覆盖初始读取/搜索意图导致的 L2 意图一致性漏报。
        // 与 AgentMoss 接口契约 docs/INTEGRATION_GUIDE.md 第 4 节一致。
        let mut prompt_history: Vec<Value> = Vec::new();
        for m in messages.iter() {
            if m.role == MessageRole::User {
                let text = m
                    .blocks
                    .iter()
                    .find_map(|b| match b {
                        agent_types::llm::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                prompt_history.push(json!({
                    "text": text,
                    "actions": Vec::<Value>::new(),
                }));
            } else {
                // 非 User 消息中的 ToolResult 归入当前（最近一个）turn 的 actions
                for block in &m.blocks {
                    if let agent_types::llm::ContentBlock::ToolResult {
                        call_id,
                        tool_name,
                        output,
                        is_error,
                    } = block
                    {
                        if let Some(last) = prompt_history.last_mut() {
                            if let Some(obj) = last.as_object_mut() {
                                let mut action = tool_use_map.get(call_id).cloned().unwrap_or_else(
                                    || json!({"action_type": tool_name, "action_detail": ""}),
                                );
                                if let Some(aobj) = action.as_object_mut() {
                                    aobj.insert("call_id".to_string(), json!(call_id));
                                    aobj.insert("output".to_string(), json!(output));
                                    aobj.insert("is_error".to_string(), json!(is_error));
                                }
                                if let Some(arr) =
                                    obj.get_mut("actions").and_then(|a| a.as_array_mut())
                                {
                                    arr.push(action);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(json!({
            "stage": "pre",
            "session_id": session_id,
            "prompt_session": prompt_session,
            "prompt_history": prompt_history,
            "action_history": action_history,
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
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
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "post",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
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
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "error",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
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

    fn serialize_metadata(&self, metadata: &HookInvokeMetadata) -> Value {
        json!({
            "trace_id": metadata.trace_id,
            "span_id": metadata.span_id,
            "parent_span_id": metadata.parent_span_id,
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

    async fn run_plugin_command(&self, payload: &Value) -> Result<Value, ToolExecutionError> {
        self.run_plugin_command_with_timeout(payload, PLUGIN_HOOKER_TIMEOUT)
            .await
    }

    async fn run_plugin_command_with_timeout(
        &self,
        payload: &Value,
        cmd_timeout: Duration,
    ) -> Result<Value, ToolExecutionError> {
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
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to spawn plugin command for hooker '{}' (command='{}'): {}",
                    self.id.0, self.command, error
                ),
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&payload_bytes).await.map_err(|error| {
                ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "failed to write stdin for plugin hooker '{}' (command='{}'): {}",
                        self.id.0, self.command, error
                    ),
                }
            })?;
        }

        let output = timeout(cmd_timeout, child.wait_with_output())
            .await
            .map_err(|_| ToolExecutionError::Timeout {
                timeout_ms: cmd_timeout.as_millis() as u64,
            })?
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

    fn parse_plugin_command_response(
        &self,
        output: Value,
    ) -> Result<PluginCommandResponse, ToolExecutionError> {
        match output.get("action").and_then(Value::as_str) {
            None | Some("final") => Ok(PluginCommandResponse::Final(output)),
            Some("ask_user") => {
                let request = serde_json::from_value(
                    self.read_required_value_field(&output, "request")?.clone(),
                )
                .map_err(|error| ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "plugin hooker '{}' ask_user request is invalid: {}",
                        self.id.0, error
                    ),
                })?;
                let continuation = self
                    .read_required_value_field(&output, "continuation")?
                    .clone();
                Ok(PluginCommandResponse::AskUser(AskUserDirective {
                    request,
                    continuation,
                }))
            }
            Some(other) => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' returned unsupported action '{}'",
                    self.id.0, other
                ),
            }),
        }
    }

    fn with_hooker_interaction_source(&self, request: PluginAskUserRequest) -> InteractionRequest {
        let source = Some(InteractionSource::Hooker {
            hooker_name: self.id.0.clone(),
            hook_point: self.hook_point.0.clone(),
        });

        match request {
            PluginAskUserRequest::Confirm { prompt } => {
                InteractionRequest::Confirm { prompt, source }
            }
            PluginAskUserRequest::TextInput { prompt } => {
                InteractionRequest::TextInput {
                    prompt,
                    source,
                    is_secret: false, // Default to false for plugin requests
                }
            }
            PluginAskUserRequest::Choice {
                prompt,
                options,
                allow_custom_input,
            } => InteractionRequest::Choice {
                prompt,
                options,
                allow_custom_input,
                source,
            },
        }
    }

    fn build_interaction_followup_payload(
        &self,
        payload: Value,
        continuation: Value,
        request: &InteractionRequest,
        response: &InteractionResponse,
    ) -> Result<Value, ToolExecutionError> {
        let mut payload_map = match payload {
            Value::Object(map) => map,
            _ => {
                return Err(ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "plugin hooker '{}' follow-up payload must be a JSON object",
                        self.id.0
                    ),
                });
            }
        };

        payload_map.insert(
            "interaction".to_string(),
            json!({
                "request": request,
                "response": response,
                "continuation": continuation,
            }),
        );
        Ok(Value::Object(payload_map))
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
                extra: output.get("extra").cloned(),
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

#[cfg(test)]
#[path = "../../../../../../tests/unit/hook/hookers/plugin/tool/adaptor_test.rs"]
mod tests;
