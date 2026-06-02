use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use crate::convert::{parse_tool_arguments, parsed_chunk_to_stream_chunk, wire_usage_to_usage};
use crate::error::{map_api_status_error, map_reqwest_error, LlmError};
use crate::wire_types::{ParsedChunk, WireUsage};
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::{
    AssistantMessage, LlmRequest, LlmResponse, StopReason, StreamChunk, ToolUseBlock, Usage,
};

mod convert;
mod types;

use convert::{
    build_gemini_request_body, extract_gemini_text, extract_gemini_tool_call_deltas,
    extract_gemini_tool_calls, normalize_model_name,
};
use types::GeminiResponseBody;

pub(crate) struct GeminiProvider {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
    capabilities: ProviderCapabilities,
    api_key_provider: Option<crate::factory::ApiKeyProviderFn>,
}

impl GeminiProvider {
    pub(crate) fn new(
        api_key: Option<String>,
        base_url: String,
        model: String,
        api_key_provider: Option<crate::factory::ApiKeyProviderFn>,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_key,
            base_url,
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: true,
                supports_json_mode: true,
                max_context_window: 1000000,
                model_name: model,
            },
            api_key_provider,
        }
    }

    fn get_api_key(&self) -> String {
        if let Some(provider) = &self.api_key_provider {
            provider()
        } else {
            self.api_key.clone().unwrap_or_default()
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let model_name = normalize_model_name(&self.capabilities.model_name);
        let api_key = self.get_api_key();
        let url = format!(
            "{}/v1beta/{}:generateContent?key={}",
            self.base_url, model_name, api_key
        );
        let body = build_gemini_request_body(request, &self.capabilities.model_name);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let resp_body = response.text().await.unwrap_or_default();
            return Err(map_api_status_error(
                status,
                &resp_body,
                &body_str,
                Some(&headers),
            ));
        }

        let gemini_response: GeminiResponseBody =
            response.json().await.map_err(map_reqwest_error)?;
        let parts = gemini_response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.content.as_ref())
            .map(|c| c.parts.as_slice())
            .unwrap_or(&[]);

        let content = extract_gemini_text(parts);
        let tool_calls = extract_gemini_tool_calls(parts);

        let finish_reason = gemini_response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.finish_reason.clone());

        let stop_reason = match finish_reason.as_deref() {
            Some("STOP") => StopReason::EndTurn,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let (prompt_tokens, completion_tokens) = gemini_response
            .usage_metadata
            .as_ref()
            .map(|u| {
                (
                    u.prompt_token_count.unwrap_or(0) as usize,
                    u.candidates_token_count.unwrap_or(0) as usize,
                )
            })
            .unwrap_or((0, 0));

        let tool_use_blocks: Vec<ToolUseBlock> = tool_calls
            .unwrap_or_default()
            .iter()
            .map(|tc| ToolUseBlock {
                call_id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                input: parse_tool_arguments(&tc.function.arguments),
            })
            .collect();

        Ok(LlmResponse {
            message: AssistantMessage {
                text: if tool_use_blocks.is_empty() {
                    content
                } else {
                    None
                },
                reasoning_content: None,
                tool_calls: tool_use_blocks,
                usage: Usage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens: prompt_tokens + completion_tokens,
                },
                stop_reason,
            },
            kv_cache_chunk_hashes: vec![],
        })
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let model_name = normalize_model_name(&self.capabilities.model_name);
        let api_key = self.get_api_key();
        let url = format!(
            "{}/v1beta/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, model_name, api_key
        );
        let body = build_gemini_request_body(request, &self.capabilities.model_name);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
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
        let mut full_tool_calls: Vec<crate::wire_types::WireToolCall> = Vec::new();
        let mut final_usage = None;
        let mut final_stop_reason = StopReason::EndTurn;

        let mut buffer = String::new();
        let mut byte_stream = response.bytes_stream();

        while let Some(chunk_result) = byte_stream.next().await {
            let bytes = chunk_result.map_err(|e| LlmError::StreamError {
                message: e.to_string(),
            })?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].to_string();
                buffer = buffer[pos + 1..].to_string();
                if line.is_empty() {
                    continue;
                }

                let data = line.strip_prefix("data: ").unwrap_or(&line).trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }

                let gemini_resp: GeminiResponseBody = match serde_json::from_str(data) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                let parts = gemini_resp
                    .candidates
                    .as_ref()
                    .and_then(|c| c.first())
                    .and_then(|c| c.content.as_ref())
                    .map(|c| c.parts.as_slice())
                    .unwrap_or(&[]);

                let content = extract_gemini_text(parts);
                let tool_call_deltas = extract_gemini_tool_call_deltas(parts);

                if let Some(ref c) = content {
                    full_text.push_str(c);
                }

                let finish_reason = gemini_resp
                    .candidates
                    .as_ref()
                    .and_then(|c| c.first())
                    .and_then(|c| c.finish_reason.clone());

                if let Some(ref r) = finish_reason {
                    final_stop_reason = match r.as_str() {
                        "STOP" => StopReason::EndTurn,
                        "MAX_TOKENS" => StopReason::MaxTokens,
                        _ => StopReason::EndTurn,
                    };
                }

                let usage = gemini_resp.usage_metadata.as_ref().map(|u| WireUsage {
                    prompt_tokens: u.prompt_token_count.unwrap_or(0),
                    completion_tokens: u.candidates_token_count.unwrap_or(0),
                    total_tokens: u.prompt_token_count.unwrap_or(0)
                        + u.candidates_token_count.unwrap_or(0),
                });
                if let Some(ref u) = usage {
                    final_usage = Some(wire_usage_to_usage(u));
                }

                let parsed = ParsedChunk {
                    content,
                    reasoning: None,
                    finish_reason,
                    usage,
                    tool_calls: tool_call_deltas,
                    kv_transfer_params: None,
                };

                super::openai_family::accumulate_tool_call_deltas_pub(
                    &mut full_tool_calls,
                    &parsed,
                );
                on_chunk(parsed_chunk_to_stream_chunk(&parsed));
            }
        }

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
                reasoning_content: None,
                tool_calls: tool_use_blocks,
                usage: final_usage.unwrap_or_default(),
                stop_reason: final_stop_reason,
            },
            kv_cache_chunk_hashes: vec![],
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}
