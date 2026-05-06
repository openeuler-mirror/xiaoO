use std::any::Any;
use std::io::Write;
use std::process::{Command, Stdio};

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::interaction::types::InteractionSource;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::llm::{
    AssistantMessage, ErrorLlmHookInput, ErrorLlmHookResult, LlmError, LlmRequest, LlmResponse,
    PostLlmHookInput, PostLlmHookResult, PreLlmHookInput, PreLlmHookResult, StopReason,
    ToolUseBlock, Usage,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{resolve_hook_point_category, HookPointCategory};

pub(crate) struct PluginLlmHookerAdaptor {
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
#[allow(dead_code)]
enum PluginAskUserRequest {
    Confirm {
        prompt: String,
        #[serde(default)]
        source: Option<InteractionSource>,
    },
    TextInput {
        prompt: String,
        #[serde(default)]
        source: Option<InteractionSource>,
    },
    Choice {
        prompt: String,
        options: Vec<String>,
        allow_custom_input: bool,
        #[serde(default)]
        source: Option<InteractionSource>,
    },
}

impl PluginLlmHookerAdaptor {
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
    ) -> Result<HookInvokeOutput, LlmError> {
        match (category, input) {
            (HookPointCategory::LlmPre, HookInvokeInput::LlmPre { input, metadata }) => {
                self.invoke_pre(&input, &metadata, runtime).await
            }
            (HookPointCategory::LlmPost, HookInvokeInput::LlmPost { input, metadata }) => {
                self.invoke_post(&input, &metadata, runtime).await
            }
            (HookPointCategory::LlmError, HookInvokeInput::LlmError { input, metadata }) => {
                self.invoke_error(&input, &metadata, runtime).await
            }
            (category, _) => Err(LlmError::RequestFailed {
                message: format!(
                    "hooker '{}' received mismatched invoke input for category {:?}",
                    self.id.0, category
                ),
            }),
        }
    }

    async fn invoke_pre(
        &self,
        input: &PreLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, LlmError> {
        let payload = self.build_pre_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::LlmPre(self.parse_pre_result(&output)?))
    }

    async fn invoke_post(
        &self,
        input: &PostLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, LlmError> {
        let payload = self.build_post_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::LlmPost(self.parse_post_result(&output)?))
    }

    async fn invoke_error(
        &self,
        input: &ErrorLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, LlmError> {
        let payload = self.build_error_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::LlmError(
            self.parse_error_result(&output)?,
        ))
    }

    async fn resolve_plugin_output(
        &self,
        initial_payload: Value,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        let mut payload = initial_payload;

        loop {
            let output = self.run_plugin_command(&payload)?;
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
        input: &PreLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        Ok(json!({
            "stage": "pre",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "request": serde_json::to_value(&input.request).map_err(|error| LlmError::RequestFailed {
                message: format!("failed to serialize pre-hook request payload for '{}': {}", self.id.0, error),
            })?,
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn build_post_payload(
        &self,
        input: &PostLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        Ok(json!({
            "stage": "post",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "request": serde_json::to_value(&input.request).map_err(|error| LlmError::RequestFailed {
                message: format!("failed to serialize post-hook request payload for '{}': {}", self.id.0, error),
            })?,
            "response": self.serialize_llm_response(&input.response),
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn build_error_payload(
        &self,
        input: &ErrorLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        Ok(json!({
            "stage": "error",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "request": serde_json::to_value(&input.request).map_err(|error| LlmError::RequestFailed {
                message: format!("failed to serialize error-hook request payload for '{}': {}", self.id.0, error),
            })?,
            "error": self.serialize_llm_error(&input.error),
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

    fn serialize_llm_response(&self, response: &LlmResponse) -> Value {
        json!({
            "message": {
                "text": &response.message.text,
                "tool_calls": response.message.tool_calls.iter().map(|tc| json!({
                    "call_id": &tc.call_id,
                    "tool_name": &tc.tool_name,
                    "input": &tc.input,
                })).collect::<Vec<_>>(),
                "usage": {
                    "prompt_tokens": response.message.usage.prompt_tokens,
                    "completion_tokens": response.message.usage.completion_tokens,
                    "total_tokens": response.message.usage.total_tokens,
                },
                "stop_reason": match &response.message.stop_reason {
                    StopReason::EndTurn => "end_turn",
                    StopReason::MaxTokens => "max_tokens",
                    StopReason::ToolUse => "tool_use",
                    StopReason::ContentFilter => "content_filter",
                },
            }
        })
    }

    fn serialize_llm_error(&self, error: &LlmError) -> Value {
        match error {
            LlmError::RequestFailed { message } => json!({
                "type": "request_failed",
                "message": message,
            }),
            LlmError::HttpError(msg) => json!({
                "type": "http_error",
                "message": msg,
            }),
            LlmError::ApiError(msg) => json!({
                "type": "api_error",
                "message": msg,
            }),
            LlmError::ParseError(msg) => json!({
                "type": "parse_error",
                "message": msg,
            }),
            LlmError::RateLimited { retry_after_ms, .. } => json!({
                "type": "rate_limited",
                "retry_after_ms": retry_after_ms,
                "message": "rate limited",
            }),
            LlmError::AuthError { message } => json!({
                "type": "auth_error",
                "message": message,
            }),
            LlmError::ModelNotFound { model } => json!({
                "type": "model_not_found",
                "model": model,
                "message": error.to_string(),
            }),
            LlmError::ProviderNotFound(msg) => json!({
                "type": "provider_not_found",
                "message": msg,
            }),
            LlmError::ConfigError(msg) => json!({
                "type": "config_error",
                "message": msg,
            }),
            LlmError::ContextLengthExceeded { message } => json!({
                "type": "context_length_exceeded",
                "message": message,
            }),
            LlmError::StreamError { message } => json!({
                "type": "stream_error",
                "message": message,
            }),
            LlmError::IoError(msg) => json!({
                "type": "io_error",
                "message": msg,
            }),
            LlmError::Timeout => json!({
                "type": "timeout",
                "message": "timeout",
            }),
            LlmError::Cancelled => json!({
                "type": "cancelled",
                "message": "cancelled",
            }),
        }
    }

    fn run_plugin_command(&self, payload: &Value) -> Result<Value, LlmError> {
        let payload_bytes =
            serde_json::to_vec(payload).map_err(|error| LlmError::RequestFailed {
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
            .map_err(|error| LlmError::RequestFailed {
                message: format!(
                    "failed to spawn plugin command for hooker '{}' (command='{}'): {}",
                    self.id.0, self.command, error
                ),
            })?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(&payload_bytes)
                .map_err(|error| LlmError::RequestFailed {
                    message: format!(
                        "failed to write stdin for plugin hooker '{}' (command='{}'): {}",
                        self.id.0, self.command, error
                    ),
                })?;
        }

        let output = child
            .wait_with_output()
            .map_err(|error| LlmError::RequestFailed {
                message: format!(
                    "failed to wait for plugin hooker '{}' (command='{}'): {}",
                    self.id.0, self.command, error
                ),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(LlmError::RequestFailed {
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

        serde_json::from_slice(&output.stdout).map_err(|error| LlmError::RequestFailed {
            message: format!(
                "plugin hooker '{}' command '{}' returned invalid JSON: {}",
                self.id.0, self.command, error
            ),
        })
    }

    fn parse_plugin_command_response(
        &self,
        output: Value,
    ) -> Result<PluginCommandResponse, LlmError> {
        match output.get("action").and_then(Value::as_str) {
            None | Some("final") => Ok(PluginCommandResponse::Final(output)),
            Some("ask_user") => {
                let request = serde_json::from_value(
                    self.read_required_value_field(&output, "request")?.clone(),
                )
                .map_err(|error| LlmError::RequestFailed {
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
            Some(other) => Err(LlmError::RequestFailed {
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
            PluginAskUserRequest::Confirm { prompt, source: _ } => {
                InteractionRequest::Confirm { prompt, source }
            }
            PluginAskUserRequest::TextInput { prompt, source: _ } => {
                InteractionRequest::TextInput { prompt, source }
            }
            PluginAskUserRequest::Choice {
                prompt,
                options,
                allow_custom_input,
                source: _,
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
    ) -> Result<Value, LlmError> {
        let mut payload_map = match payload {
            Value::Object(map) => map,
            _ => {
                return Err(LlmError::RequestFailed {
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

    fn parse_pre_result(&self, output: &Value) -> Result<PreLlmHookResult, LlmError> {
        match self.read_required_result_tag(output)?.as_str() {
            "allow" => Ok(PreLlmHookResult::Allow),
            "transform" => {
                let modified_request_value =
                    self.read_required_value_field(output, "modified_request")?;
                let modified_request: LlmRequest =
                    serde_json::from_value(modified_request_value.clone()).map_err(|error| {
                        LlmError::RequestFailed {
                            message: format!(
                                "plugin llm pre-hooker '{}' returned invalid modified_request: {}",
                                self.id.0, error
                            ),
                        }
                    })?;
                Ok(PreLlmHookResult::Transform { modified_request })
            }
            result => Err(LlmError::RequestFailed {
                message: format!(
                    "plugin llm pre-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_post_result(&self, output: &Value) -> Result<PostLlmHookResult, LlmError> {
        match self.read_required_result_tag(output)?.as_str() {
            "accept" => Ok(PostLlmHookResult::Accept),
            "transform" => {
                let modified_response_value =
                    self.read_required_value_field(output, "modified_response")?;
                let modified_response =
                    self.parse_llm_response_from_value(modified_response_value)?;
                Ok(PostLlmHookResult::Transform { modified_response })
            }
            result => Err(LlmError::RequestFailed {
                message: format!(
                    "plugin llm post-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_error_result(&self, output: &Value) -> Result<ErrorLlmHookResult, LlmError> {
        match self.read_required_result_tag(output)?.as_str() {
            "propagate" => Ok(ErrorLlmHookResult::Propagate),
            "recover" => {
                let response_value = self.read_required_value_field(output, "response")?;
                let response = self.parse_llm_response_from_value(response_value)?;
                Ok(ErrorLlmHookResult::Recover { response })
            }
            result => Err(LlmError::RequestFailed {
                message: format!(
                    "plugin llm error-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_llm_response_from_value(&self, value: &Value) -> Result<LlmResponse, LlmError> {
        let message_value = value
            .get("message")
            .ok_or_else(|| LlmError::RequestFailed {
                message: format!(
                    "plugin llm hooker '{}' response must contain 'message' field",
                    self.id.0
                ),
            })?;

        let text = message_value
            .get("text")
            .and_then(Value::as_str)
            .map(String::from);

        let tool_calls = self.parse_tool_calls(message_value)?;

        let usage = self.parse_usage(message_value)?;

        let stop_reason = self.parse_stop_reason(message_value)?;

        Ok(LlmResponse {
            message: AssistantMessage {
                text,
                reasoning_content: message_value
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .map(String::from),
                tool_calls,
                usage,
                stop_reason,
            },
        })
    }

    fn parse_tool_calls(&self, message_value: &Value) -> Result<Vec<ToolUseBlock>, LlmError> {
        let tool_calls_value =
            message_value
                .get("tool_calls")
                .ok_or_else(|| LlmError::RequestFailed {
                    message: format!(
                        "plugin llm hooker '{}' response message must contain 'tool_calls' field",
                        self.id.0
                    ),
                })?;

        let tool_calls_array =
            tool_calls_value
                .as_array()
                .ok_or_else(|| LlmError::RequestFailed {
                    message: format!(
                        "plugin llm hooker '{}' response message 'tool_calls' must be an array",
                        self.id.0
                    ),
                })?;

        let mut tool_calls = Vec::with_capacity(tool_calls_array.len());
        for tc in tool_calls_array {
            tool_calls.push(ToolUseBlock {
                call_id: tc
                    .get("call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| LlmError::RequestFailed {
                        message: format!(
                            "plugin llm hooker '{}' tool_call must have 'call_id' string",
                            self.id.0
                        ),
                    })?
                    .to_string(),
                tool_name: tc
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| LlmError::RequestFailed {
                        message: format!(
                            "plugin llm hooker '{}' tool_call must have 'tool_name' string",
                            self.id.0
                        ),
                    })?
                    .to_string(),
                input: tc.get("input").cloned().unwrap_or(Value::Null),
            });
        }

        Ok(tool_calls)
    }

    fn parse_usage(&self, message_value: &Value) -> Result<Usage, LlmError> {
        let usage_value = message_value
            .get("usage")
            .ok_or_else(|| LlmError::RequestFailed {
                message: format!(
                    "plugin llm hooker '{}' response message must contain 'usage' field",
                    self.id.0
                ),
            })?;

        Ok(Usage {
            prompt_tokens: usage_value
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            completion_tokens: usage_value
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            total_tokens: usage_value
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        })
    }

    fn parse_stop_reason(&self, message_value: &Value) -> Result<StopReason, LlmError> {
        let stop_reason_str = message_value
            .get("stop_reason")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::RequestFailed {
                message: format!(
                    "plugin llm hooker '{}' response message must contain 'stop_reason' string",
                    self.id.0
                ),
            })?;

        match stop_reason_str {
            "end_turn" => Ok(StopReason::EndTurn),
            "max_tokens" => Ok(StopReason::MaxTokens),
            "tool_use" => Ok(StopReason::ToolUse),
            "content_filter" => Ok(StopReason::ContentFilter),
            _ => Err(LlmError::RequestFailed {
                message: format!(
                    "plugin llm hooker '{}' returned invalid stop_reason '{}'",
                    self.id.0, stop_reason_str
                ),
            }),
        }
    }

    fn read_required_result_tag(&self, output: &Value) -> Result<String, LlmError> {
        Ok(self
            .read_required_string_field(output, "result")?
            .to_lowercase())
    }

    fn read_required_string_field<'a>(
        &self,
        output: &'a Value,
        field_name: &str,
    ) -> Result<&'a str, LlmError> {
        output
            .get(field_name)
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::RequestFailed {
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
    ) -> Result<&'a Value, LlmError> {
        output
            .get(field_name)
            .ok_or_else(|| LlmError::RequestFailed {
                message: format!(
                    "plugin hooker '{}' response must contain field '{}'",
                    self.id.0, field_name
                ),
            })
    }
}

#[async_trait]
impl Hooker for PluginLlmHookerAdaptor {
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
            HookInvokeError::Llm(LlmError::RequestFailed {
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
