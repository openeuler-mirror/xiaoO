use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use crate::convert::{
    llm_request_to_wire, parse_tool_arguments, parsed_chunk_to_stream_chunk,
    wire_response_to_llm_response, wire_usage_to_usage,
};
use crate::error::{
    map_api_status_error, map_reqwest_error, map_serde_error, parse_stream_error, LlmError,
};
use crate::wire_types::{ChatCompletionChunk, ParsedChunk};
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::{
    AssistantMessage, LlmRequest, LlmResponse, ReasoningEffort, StopReason, StreamChunk,
    ToolUseBlock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAiFamilyAuthStyle {
    Bearer,
}

#[derive(Clone)]
pub(crate) struct OpenAiFamilyProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    auth_style: OpenAiFamilyAuthStyle,
    default_headers: Vec<(String, String)>,
    capabilities: ProviderCapabilities,
}

impl OpenAiFamilyProvider {
    pub(crate) fn new(
        api_key: String,
        api_base: String,
        model: String,
        auth_style: OpenAiFamilyAuthStyle,
        default_headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .connect_timeout(Duration::from_secs(30))
                .http1_only()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_key,
            api_base,
            auth_style,
            default_headers,
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: true,
                supports_json_mode: true,
                max_context_window: 128000,
                model_name: model,
            },
        }
    }

    fn build_body(
        &self,
        request: &LlmRequest,
        force_stream: bool,
    ) -> Result<serde_json::Value, LlmError> {
        let wire = llm_request_to_wire(request, &self.capabilities.model_name);
        let mut body = serde_json::to_value(&wire).map_err(map_serde_error)?;
        if let Some(reasoning_effort) = openai_reasoning_effort(request.reasoning_effort) {
            body["reasoning_effort"] = serde_json::json!(reasoning_effort);
        }
        if force_stream {
            body["stream"] = serde_json::json!(true);
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        Ok(body)
    }

    fn apply_common_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req.header("Content-Type", "application/json");
        req = match self.auth_style {
            OpenAiFamilyAuthStyle::Bearer => {
                req.header("Authorization", format!("Bearer {}", self.api_key))
            }
        };
        for (name, value) in &self.default_headers {
            req = req.header(name.as_str(), value);
        }
        req
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.api_base.trim_end_matches('/'))
    }
}

fn openai_reasoning_effort(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::Off => None,
        ReasoningEffort::High => Some("high"),
        ReasoningEffort::Max => Some("xhigh"),
    }
}

#[async_trait]
impl LlmProvider for OpenAiFamilyProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let url = self.chat_completions_url();
        let body = self.build_body(request, false)?;
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let response = self
            .apply_common_headers(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        let resp_body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(map_api_status_error(
                status,
                &resp_body,
                &body_str,
                Some(&headers),
            ));
        }

        let wire_response: crate::wire_types::WireResponse =
            serde_json::from_str(&resp_body).map_err(map_serde_error)?;
        let mut llm_response = wire_response_to_llm_response(&wire_response);
        llm_response.message.usage = wire_usage_to_usage(&wire_response.usage);
        Ok(llm_response)
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let url = self.chat_completions_url();
        let body = self.build_body(request, true)?;
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let response = self
            .apply_common_headers(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let error_body = response.text().await.unwrap_or_default();
            return Err(map_api_status_error(
                status,
                &error_body,
                &body_str,
                Some(&headers),
            ));
        }

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut full_tool_calls = Vec::new();
        let mut final_usage = None;
        let mut final_finish_reason = None;

        let mut buffer = String::new();
        let mut byte_stream = response.bytes_stream();

        while let Some(chunk_result) = byte_stream.next().await {
            let bytes = chunk_result.map_err(|e| LlmError::StreamError {
                message: e.to_string(),
            })?;
            let text = String::from_utf8_lossy(&bytes);
            buffer.push_str(&text);

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(parsed) = parse_openai_family_stream_line(&line)? {
                    if let Some(ref content) = parsed.content {
                        full_text.push_str(content);
                    }
                    if let Some(ref reasoning) = parsed.reasoning {
                        full_reasoning.push_str(reasoning);
                    }
                    if let Some(ref usage) = parsed.usage {
                        final_usage = Some(usage.clone());
                    }
                    if let Some(ref reason) = parsed.finish_reason {
                        final_finish_reason = Some(reason.clone());
                    }
                    accumulate_tool_call_deltas(&mut full_tool_calls, &parsed);

                    let stream_chunk = parsed_chunk_to_stream_chunk(&parsed);
                    on_chunk(stream_chunk);
                }
            }
        }

        let usage = final_usage
            .map(|u| wire_usage_to_usage(&u))
            .unwrap_or_default();

        let stop_reason = match final_finish_reason.as_deref() {
            Some("stop") | Some("end_turn") => StopReason::EndTurn,
            Some("length") | Some("max_tokens") => StopReason::MaxTokens,
            Some("tool_calls") | Some("tool_use") => StopReason::ToolUse,
            Some("content_filter") => StopReason::ContentFilter,
            _ => StopReason::EndTurn,
        };

        let tool_use_blocks: Vec<ToolUseBlock> = full_tool_calls
            .iter()
            .map(|tc| ToolUseBlock {
                call_id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                input: parse_tool_arguments(&tc.function.arguments),
            })
            .collect();

        Ok(LlmResponse {
            message: AssistantMessage {
                text: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                reasoning_content: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning)
                },
                tool_calls: tool_use_blocks,
                usage,
                stop_reason,
            },
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

pub(crate) fn parse_openai_family_stream_line(line: &str) -> Result<Option<ParsedChunk>, LlmError> {
    let line = line.trim();

    if line.is_empty() || line.starts_with(':') || line.starts_with("event: ") {
        return Ok(Some(ParsedChunk::default()));
    }

    if line == "[DONE]" {
        return Ok(None);
    }

    let data = match line.strip_prefix("data:") {
        Some(data) => data.trim(),
        None => return Ok(Some(ParsedChunk::default())),
    };

    if data.is_empty() {
        return Ok(Some(ParsedChunk::default()));
    }

    if data == "[DONE]" {
        return Ok(None);
    }

    if let Some(error) = parse_stream_error(data) {
        return Err(error);
    }

    let chunk: ChatCompletionChunk = serde_json::from_str(data)
        .map_err(|e| LlmError::ParseError(format!("Failed to parse stream chunk: {}", e)))?;

    let parsed = if let Some(choice) = chunk.choices.first() {
        ParsedChunk {
            content: choice.delta.content.clone(),
            reasoning: choice.delta.reasoning.clone(),
            finish_reason: choice.finish_reason.clone(),
            usage: chunk.usage.clone(),
            tool_calls: choice.delta.tool_calls.clone(),
        }
    } else {
        ParsedChunk {
            content: None,
            reasoning: None,
            finish_reason: None,
            usage: chunk.usage.clone(),
            tool_calls: None,
        }
    };

    Ok(Some(parsed))
}

pub(crate) fn accumulate_tool_call_deltas_pub(
    full_tool_calls: &mut Vec<crate::wire_types::WireToolCall>,
    parsed: &ParsedChunk,
) {
    accumulate_tool_call_deltas(full_tool_calls, parsed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::{ChatMessageExt, LlmRequestExt};

    fn make_provider() -> OpenAiFamilyProvider {
        OpenAiFamilyProvider::new(
            "test-key".to_string(),
            "https://api.openai.com/v1".to_string(),
            "gpt-5.4".to_string(),
            OpenAiFamilyAuthStyle::Bearer,
            vec![],
        )
    }

    #[test]
    fn build_body_sets_reasoning_effort() {
        let provider = make_provider();
        let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
            .with_reasoning_effort(ReasoningEffort::Max);

        let body = provider.build_body(&request, false).unwrap();

        assert_eq!(body["reasoning_effort"], "xhigh");
    }

    #[test]
    fn build_body_omits_reasoning_effort_when_off() {
        let provider = make_provider();
        let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
            .with_reasoning_effort(ReasoningEffort::Off);

        let body = provider.build_body(&request, false).unwrap();

        assert!(body.get("reasoning_effort").is_none());
    }
}

/// Configuration for creating an OpenAI-compatible provider directly.
pub struct OpenAiCompatibleProviderConfig {
    pub api_base: String,
    pub api_key: String,
    pub capabilities: ProviderCapabilities,
}

impl OpenAiCompatibleProviderConfig {
    pub fn new(
        api_base: impl Into<String>,
        api_key: impl Into<String>,
        capabilities: ProviderCapabilities,
    ) -> Self {
        Self {
            api_base: api_base.into(),
            api_key: api_key.into(),
            capabilities,
        }
    }
}

/// A publicly constructable OpenAI-compatible LLM provider.
pub struct OpenAiCompatibleProvider {
    inner: OpenAiFamilyProvider,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleProviderConfig) -> Result<Self, LlmError> {
        if config.api_base.is_empty() {
            return Err(LlmError::ConfigError(
                "api_base must not be empty".to_string(),
            ));
        }
        if config.api_key.is_empty() {
            return Err(LlmError::ConfigError(
                "api_key must not be empty".to_string(),
            ));
        }
        let model = config.capabilities.model_name.clone();
        let mut inner = OpenAiFamilyProvider::new(
            config.api_key,
            config.api_base,
            model,
            OpenAiFamilyAuthStyle::Bearer,
            Vec::new(),
        );
        inner.capabilities = config.capabilities;
        Ok(Self { inner })
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.inner.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        self.inner.complete_stream(request, on_chunk).await
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        self.inner.capabilities()
    }
}

fn accumulate_tool_call_deltas(
    full_tool_calls: &mut Vec<crate::wire_types::WireToolCall>,
    parsed: &ParsedChunk,
) {
    if let Some(ref deltas) = parsed.tool_calls {
        for delta in deltas {
            let idx = delta.index as usize;
            while full_tool_calls.len() <= idx {
                full_tool_calls.push(crate::wire_types::WireToolCall {
                    id: String::new(),
                    call_type: "function".to_string(),
                    function: crate::wire_types::WireToolCallFunction {
                        name: String::new(),
                        arguments: String::new(),
                    },
                });
            }
            let tc = &mut full_tool_calls[idx];
            if let Some(ref id) = delta.id {
                tc.id = id.clone();
            }
            if let Some(ref func) = delta.function {
                if let Some(ref name) = func.name {
                    tc.function.name = name.clone();
                }
                if let Some(ref args) = func.arguments {
                    tc.function.arguments.push_str(args);
                }
            }
        }
    }
}
