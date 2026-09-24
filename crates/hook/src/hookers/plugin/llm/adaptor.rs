use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::llm::{
    AssistantMessage, ErrorLlmHookInput, ErrorLlmHookResult, LlmError, LlmRequest, LlmResponse,
    PostLlmHookInput, PostLlmHookResult, PreLlmHookInput, PreLlmHookResult, StopReason,
    ToolUseBlock, Usage,
};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::core::PluginHookerCore;
use crate::{resolve_hook_point_category, HookPointCategory};

/// plugin hooker 子进程最长执行时间(10 分钟)。超时后由 `kill_on_drop` 自动兜底杀掉子进程,
/// 防止卡死命令长期阻塞 tokio worker。
const PLUGIN_HOOKER_TIMEOUT_MS: u64 = 600_000;

pub(crate) struct PluginLlmHookerAdaptor {
    core: PluginHookerCore,
}

impl PluginLlmHookerAdaptor {
    pub fn new(
        id: HookerId,
        hook_point: HookPointId,
        command: String,
        definition: serde_json::Value,
    ) -> Self {
        Self {
            core: PluginHookerCore::new(id, hook_point, command, definition),
        }
    }

    /// Lift a message into the llm-domain plugin error. Passed to
    /// [`PluginHookerCore`] helpers so the shared subprocess/JSON code paths
    /// construct `LlmError` rather than a foreign error type.
    fn err(message: String) -> LlmError {
        LlmError::RequestFailed { message }
    }

    /// LLM hook timeouts surface as the dedicated `LlmError::Timeout`
    /// variant (no message payload), matching the LLM client's timeout
    /// semantics.
    fn timeout_err(_message: String, _timeout_ms: u64) -> LlmError {
        LlmError::Timeout
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
            (category, _) => Err(Self::err(format!(
                "hooker '{}' received mismatched invoke input for category {:?}",
                self.core.id().0,
                category
            ))),
        }
    }

    async fn invoke_pre(
        &self,
        input: &PreLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, LlmError> {
        let payload = self.build_pre_payload(input, metadata, runtime)?;
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOKER_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::LlmPre(self.parse_pre_result(&output)?))
    }

    async fn invoke_post(
        &self,
        input: &PostLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, LlmError> {
        let payload = self.build_post_payload(input, metadata, runtime)?;
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOKER_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::LlmPost(self.parse_post_result(&output)?))
    }

    async fn invoke_error(
        &self,
        input: &ErrorLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, LlmError> {
        let payload = self.build_error_payload(input, metadata, runtime)?;
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOKER_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::LlmError(
            self.parse_error_result(&output)?,
        ))
    }

    fn build_pre_payload(
        &self,
        input: &PreLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        let request = serde_json::to_value(&input.request).map_err(|error| {
            Self::err(format!(
                "failed to serialize pre-hook request payload for '{}': {}",
                self.core.id().0,
                error
            ))
        })?;
        Ok(self
            .core
            .build_stage_payload("pre", metadata, runtime, vec![("request", request)]))
    }

    fn build_post_payload(
        &self,
        input: &PostLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        let request = serde_json::to_value(&input.request).map_err(|error| {
            Self::err(format!(
                "failed to serialize post-hook request payload for '{}': {}",
                self.core.id().0,
                error
            ))
        })?;
        Ok(self.core.build_stage_payload(
            "post",
            metadata,
            runtime,
            vec![
                ("request", request),
                ("response", self.serialize_llm_response(&input.response)),
            ],
        ))
    }

    fn build_error_payload(
        &self,
        input: &ErrorLlmHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, LlmError> {
        let request = serde_json::to_value(&input.request).map_err(|error| {
            Self::err(format!(
                "failed to serialize error-hook request payload for '{}': {}",
                self.core.id().0,
                error
            ))
        })?;
        Ok(self.core.build_stage_payload(
            "error",
            metadata,
            runtime,
            vec![
                ("request", request),
                ("error", self.serialize_llm_error(&input.error)),
            ],
        ))
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
                    "cached_tokens": response.message.usage.cached_tokens,
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

    fn parse_pre_result(&self, output: &Value) -> Result<PreLlmHookResult, LlmError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "allow" => Ok(PreLlmHookResult::Allow),
            "transform" => {
                let modified_request_value =
                    self.core
                        .read_required_value_field(output, "modified_request", Self::err)?;
                let modified_request: LlmRequest =
                    serde_json::from_value(modified_request_value.clone()).map_err(|error| {
                        Self::err(format!(
                            "plugin llm pre-hooker '{}' returned invalid modified_request: {}",
                            self.core.id().0,
                            error
                        ))
                    })?;
                Ok(PreLlmHookResult::Transform { modified_request })
            }
            result => Err(Self::err(format!(
                "plugin llm pre-hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    fn parse_post_result(&self, output: &Value) -> Result<PostLlmHookResult, LlmError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "accept" => Ok(PostLlmHookResult::Accept),
            "transform" => {
                let modified_response_value =
                    self.core
                        .read_required_value_field(output, "modified_response", Self::err)?;
                let modified_response =
                    self.parse_llm_response_from_value(modified_response_value)?;
                Ok(PostLlmHookResult::Transform { modified_response })
            }
            result => Err(Self::err(format!(
                "plugin llm post-hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    fn parse_error_result(&self, output: &Value) -> Result<ErrorLlmHookResult, LlmError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "propagate" => Ok(ErrorLlmHookResult::Propagate),
            "recover" => {
                let response_value =
                    self.core
                        .read_required_value_field(output, "response", Self::err)?;
                let response = self.parse_llm_response_from_value(response_value)?;
                Ok(ErrorLlmHookResult::Recover { response })
            }
            result => Err(Self::err(format!(
                "plugin llm error-hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    fn parse_llm_response_from_value(&self, value: &Value) -> Result<LlmResponse, LlmError> {
        let message_value = value.get("message").ok_or_else(|| {
            Self::err(format!(
                "plugin llm hooker '{}' response must contain 'message' field",
                self.core.id().0
            ))
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
            kv_cache_chunk_hashes: vec![],
        })
    }

    fn parse_tool_calls(&self, message_value: &Value) -> Result<Vec<ToolUseBlock>, LlmError> {
        let tool_calls_value = message_value.get("tool_calls").ok_or_else(|| {
            Self::err(format!(
                "plugin llm hooker '{}' response message must contain 'tool_calls' field",
                self.core.id().0
            ))
        })?;

        let tool_calls_array = tool_calls_value.as_array().ok_or_else(|| {
            Self::err(format!(
                "plugin llm hooker '{}' response message 'tool_calls' must be an array",
                self.core.id().0
            ))
        })?;

        let mut tool_calls = Vec::with_capacity(tool_calls_array.len());
        for tc in tool_calls_array {
            tool_calls.push(ToolUseBlock {
                call_id: tc
                    .get("call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Self::err(format!(
                            "plugin llm hooker '{}' tool_call must have 'call_id' string",
                            self.core.id().0
                        ))
                    })?
                    .to_string(),
                tool_name: tc
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Self::err(format!(
                            "plugin llm hooker '{}' tool_call must have 'tool_name' string",
                            self.core.id().0
                        ))
                    })?
                    .to_string(),
                input: tc.get("input").cloned().unwrap_or(Value::Null),
            });
        }

        Ok(tool_calls)
    }

    fn parse_usage(&self, message_value: &Value) -> Result<Usage, LlmError> {
        let usage_value = message_value.get("usage").ok_or_else(|| {
            Self::err(format!(
                "plugin llm hooker '{}' response message must contain 'usage' field",
                self.core.id().0
            ))
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
            cached_tokens: usage_value
                .get("cached_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        })
    }

    fn parse_stop_reason(&self, message_value: &Value) -> Result<StopReason, LlmError> {
        let stop_reason_str = message_value
            .get("stop_reason")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Self::err(format!(
                    "plugin llm hooker '{}' response message must contain 'stop_reason' string",
                    self.core.id().0
                ))
            })?;

        match stop_reason_str {
            "end_turn" => Ok(StopReason::EndTurn),
            "max_tokens" => Ok(StopReason::MaxTokens),
            "tool_use" => Ok(StopReason::ToolUse),
            "content_filter" => Ok(StopReason::ContentFilter),
            _ => Err(Self::err(format!(
                "plugin llm hooker '{}' returned invalid stop_reason '{}'",
                self.core.id().0,
                stop_reason_str
            ))),
        }
    }
}

#[async_trait]
impl Hooker for PluginLlmHookerAdaptor {
    fn id(&self) -> &HookerId {
        self.core.id()
    }

    fn hook_point(&self) -> &HookPointId {
        self.core.hook_point()
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let category = resolve_hook_point_category(self.core.hook_point()).map_err(|error| {
            HookInvokeError::Llm(Self::err(format!(
                "failed to resolve hook point category for hooker '{}': {}",
                self.core.id().0,
                error
            )))
        })?;

        self.invoke_for_category(category, input, runtime)
            .await
            .map_err(HookInvokeError::from)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
