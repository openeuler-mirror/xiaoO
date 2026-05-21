use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use crate::convert::{parse_tool_arguments, parsed_chunk_to_stream_chunk, wire_usage_to_usage};
use crate::error::{map_api_status_error, map_reqwest_error, map_serde_error, LlmError};
use crate::wire_types::{ParsedChunk, WireToolCallDelta, WireToolCallFunctionDelta, WireUsage};
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::{
    AssistantMessage, LlmRequest, LlmResponse, ReasoningEffort, StopReason, StreamChunk,
    ToolUseBlock, Usage,
};

mod convert;

use convert::{
    anthropic_messages, anthropic_system_message, extract_anthropic_tool_calls,
    to_anthropic_output_format, to_anthropic_tool, to_anthropic_tool_choice,
};

pub(crate) struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    capabilities: ProviderCapabilities,
    current_event: Mutex<Option<String>>,
}

impl AnthropicProvider {
    pub(crate) fn new(api_key: String, base_url: String, model: String) -> Self {
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
                max_context_window: 200000,
                model_name: model,
            },
            current_event: Mutex::new(None),
        }
    }

    fn build_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        let system_message = anthropic_system_message(&request.messages);
        let other_messages = anthropic_messages(&request.messages);

        let max_tokens = request.max_tokens.unwrap_or(16384);
        let (max_tokens, thinking_budget) =
            anthropic_reasoning_budget(request.reasoning_effort, max_tokens);

        let mut body = serde_json::json!({
            "model": self.capabilities.model_name,
            "messages": other_messages,
            "max_tokens": max_tokens,
        });

        if stream {
            body["stream"] = serde_json::json!(true);
        }

        if !system_message.is_empty() {
            body["system"] = serde_json::json!(system_message);
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::json!(request
                .tools
                .iter()
                .map(|t| to_anthropic_tool(t))
                .collect::<Vec<_>>());
        }

        let wire_tool_choice = crate::convert::tool_choice_to_wire(&request.tool_choice);
        body["tool_choice"] = to_anthropic_tool_choice(&wire_tool_choice);

        let wire_format = crate::convert::response_format_to_wire(&request.response_format);
        if let Some(ref wf) = wire_format {
            if let Some(output_format) = to_anthropic_output_format(wf) {
                body["output_config"] = serde_json::json!({ "format": output_format });
            }
        }

        if let Some(budget_tokens) = thinking_budget {
            body["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget_tokens,
            });
        }

        body
    }
}

fn anthropic_reasoning_budget(
    effort: ReasoningEffort,
    requested_max_tokens: usize,
) -> (usize, Option<usize>) {
    match effort {
        ReasoningEffort::Off => (requested_max_tokens, None),
        ReasoningEffort::High | ReasoningEffort::Max => {
            let max_tokens = requested_max_tokens.max(2048);
            let divisor = if effort == ReasoningEffort::High {
                4
            } else {
                2
            };
            let cap = if effort == ReasoningEffort::High {
                8192
            } else {
                32768
            };
            let budget = (max_tokens / divisor).clamp(1024, cap).min(max_tokens - 1);
            (max_tokens, Some(budget))
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let url = format!("{}/messages", self.base_url);
        let body = self.build_body(request, false);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
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

        let anthropic_response: serde_json::Value =
            serde_json::from_str(&resp_body).map_err(map_serde_error)?;

        let content = anthropic_response["content"]
            .as_array()
            .and_then(|arr| arr.iter().find_map(|c| c["text"].as_str()))
            .unwrap_or("")
            .to_string();
        let reasoning_content = anthropic_response["content"].as_array().and_then(|arr| {
            let thinking = arr
                .iter()
                .filter_map(|c| c["thinking"].as_str())
                .collect::<Vec<_>>()
                .join("");
            (!thinking.is_empty()).then_some(thinking)
        });
        let tool_calls = extract_anthropic_tool_calls(&anthropic_response["content"]);

        let usage_val = &anthropic_response["usage"];
        let usage = Usage {
            prompt_tokens: usage_val["input_tokens"].as_u64().unwrap_or(0) as usize,
            completion_tokens: usage_val["output_tokens"].as_u64().unwrap_or(0) as usize,
            total_tokens: (usage_val["input_tokens"].as_u64().unwrap_or(0)
                + usage_val["output_tokens"].as_u64().unwrap_or(0))
                as usize,
        };

        let finish_reason = anthropic_response["stop_reason"]
            .as_str()
            .unwrap_or("end_turn");
        let stop_reason = match finish_reason {
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let tool_use_blocks: Vec<ToolUseBlock> = tool_calls
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
                    Some(content)
                } else {
                    None
                },
                reasoning_content,
                tool_calls: tool_use_blocks,
                usage,
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
        let url = format!("{}/messages", self.base_url);
        let body = self.build_body(request, true);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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
        let mut full_reasoning = String::new();
        let mut full_tool_calls: Vec<crate::wire_types::WireToolCall> = Vec::new();
        let mut final_usage = None;
        let mut final_stop_reason = StopReason::EndTurn;

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

                if let Some(parsed) = self.parse_anthropic_stream_line(&line)? {
                    if let Some(ref content) = parsed.content {
                        full_text.push_str(content);
                    }
                    if let Some(ref reasoning) = parsed.reasoning {
                        full_reasoning.push_str(reasoning);
                    }
                    if let Some(ref usage) = parsed.usage {
                        final_usage =
                            Some(merge_usage(final_usage.take(), wire_usage_to_usage(usage)));
                    }
                    if let Some(ref reason) = parsed.finish_reason {
                        final_stop_reason = match reason.as_str() {
                            "tool_use" => StopReason::ToolUse,
                            "max_tokens" => StopReason::MaxTokens,
                            _ => StopReason::EndTurn,
                        };
                    }
                    super::openai_family::accumulate_tool_call_deltas_pub(
                        &mut full_tool_calls,
                        &parsed,
                    );

                    let stream_chunk = parsed_chunk_to_stream_chunk(&parsed);
                    on_chunk(stream_chunk);
                }
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
                reasoning_content: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning)
                },
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

impl AnthropicProvider {
    fn parse_anthropic_stream_line(&self, line: &str) -> Result<Option<ParsedChunk>, LlmError> {
        if let Some(event_type) = line.strip_prefix("event: ") {
            *self.current_event.lock().unwrap() = Some(event_type.to_string());
            return Ok(Some(ParsedChunk::default()));
        }

        let data = match line.strip_prefix("data: ") {
            Some(d) => d,
            None => return Ok(Some(ParsedChunk::default())),
        };

        let json: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(Some(ParsedChunk::default())),
        };

        let event_type = self.current_event.lock().unwrap().clone();

        match event_type.as_deref() {
            Some("message_start") => {
                let input_tokens = json["message"]["usage"]["input_tokens"]
                    .as_u64()
                    .or_else(|| json["usage"]["input_tokens"].as_u64())
                    .map(|t| t as u32);
                let usage = input_tokens.map(|t| WireUsage {
                    prompt_tokens: t,
                    completion_tokens: 0,
                    total_tokens: t,
                });
                Ok(Some(ParsedChunk {
                    content: None,
                    reasoning: None,
                    finish_reason: None,
                    usage,
                    tool_calls: None,
                    kv_transfer_params: None,
                }))
            }
            Some("content_block_delta") => {
                let delta_type = json["delta"]["type"].as_str();
                let text = json["delta"]["text"].as_str().map(|s| s.to_string());
                let reasoning = json["delta"]["thinking"].as_str().map(|s| s.to_string());
                let tool_calls = if delta_type == Some("input_json_delta") {
                    Some(vec![WireToolCallDelta {
                        index: json["index"].as_u64().unwrap_or(0) as u32,
                        id: None,
                        call_type: Some("function".to_string()),
                        function: Some(WireToolCallFunctionDelta {
                            name: None,
                            arguments: json["delta"]["partial_json"]
                                .as_str()
                                .map(|s| s.to_string()),
                        }),
                    }])
                } else {
                    None
                };
                Ok(Some(ParsedChunk {
                    content: text,
                    reasoning,
                    finish_reason: None,
                    usage: None,
                    tool_calls,
                    kv_transfer_params: None,
                }))
            }
            Some("content_block_start") => {
                if json["content_block"]["type"].as_str() == Some("tool_use") {
                    Ok(Some(ParsedChunk {
                        content: None,
                        reasoning: None,
                        finish_reason: None,
                        usage: None,
                        tool_calls: Some(vec![WireToolCallDelta {
                            index: json["index"].as_u64().unwrap_or(0) as u32,
                            id: json["content_block"]["id"].as_str().map(|s| s.to_string()),
                            call_type: Some("function".to_string()),
                            function: Some(WireToolCallFunctionDelta {
                                name: json["content_block"]["name"]
                                    .as_str()
                                    .map(|s| s.to_string()),
                                arguments: None,
                            }),
                        }]),
                        kv_transfer_params: None,
                    }))
                } else {
                    Ok(Some(ParsedChunk::default()))
                }
            }
            Some("content_block_stop") => Ok(Some(ParsedChunk::default())),
            Some("message_delta") => {
                let stop_reason = json["delta"]["stop_reason"].as_str().map(|s| s.to_string());
                let output_tokens = json["usage"]["output_tokens"].as_u64().map(|t| t as u32);
                let usage = output_tokens.map(|t| WireUsage {
                    prompt_tokens: 0,
                    completion_tokens: t,
                    total_tokens: t,
                });
                Ok(Some(ParsedChunk {
                    content: None,
                    reasoning: None,
                    finish_reason: stop_reason,
                    usage,
                    tool_calls: None,
                    kv_transfer_params: None,
                }))
            }
            Some("message_stop") => Ok(None),
            _ => Ok(Some(ParsedChunk::default())),
        }
    }
}

fn merge_usage(existing: Option<Usage>, incoming: Usage) -> Usage {
    let mut merged = existing.unwrap_or_default();
    merged.prompt_tokens = merged.prompt_tokens.max(incoming.prompt_tokens);
    merged.completion_tokens = merged.completion_tokens.max(incoming.completion_tokens);
    merged.total_tokens = merged.prompt_tokens + merged.completion_tokens;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::{ChatMessageExt, LlmRequestExt};
    use agent_types::LlmRequest;

    fn make_provider() -> AnthropicProvider {
        AnthropicProvider::new(
            "test-key".to_string(),
            "https://api.anthropic.com/v1".to_string(),
            "claude-sonnet-4-6".to_string(),
        )
    }

    #[test]
    fn test_parse_content_block_delta() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: content_block_delta")
            .unwrap();
        let result = provider.parse_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        ).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().content, Some("Hello".to_string()));
    }

    #[test]
    fn test_parse_message_delta() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: message_delta")
            .unwrap();
        let result = provider.parse_anthropic_stream_line(
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#,
        ).unwrap();
        let chunk = result.unwrap();
        assert_eq!(chunk.finish_reason, Some("end_turn".to_string()));
        assert!(chunk.usage.is_some());
        assert_eq!(chunk.usage.unwrap().completion_tokens, 15);
    }

    #[test]
    fn test_parse_message_start_usage() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: message_start")
            .unwrap();
        let result = provider
            .parse_anthropic_stream_line(
                r#"data: {"type":"message_start","message":{"usage":{"input_tokens":21}}}"#,
            )
            .unwrap();
        let chunk = result.unwrap();
        assert!(chunk.usage.is_some());
        assert_eq!(chunk.usage.unwrap().prompt_tokens, 21);
    }

    #[test]
    fn merge_usage_keeps_prompt_and_completion_totals() {
        let merged = merge_usage(
            Some(Usage {
                prompt_tokens: 21,
                completion_tokens: 0,
                total_tokens: 21,
            }),
            Usage {
                prompt_tokens: 0,
                completion_tokens: 15,
                total_tokens: 15,
            },
        );

        assert_eq!(merged.prompt_tokens, 21);
        assert_eq!(merged.completion_tokens, 15);
        assert_eq!(merged.total_tokens, 36);
    }

    #[test]
    fn test_parse_message_stop() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: message_stop")
            .unwrap();
        let result = provider
            .parse_anthropic_stream_line(r#"data: {"type":"message_stop"}"#)
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_input_json_delta_as_tool_call() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: content_block_delta")
            .unwrap();
        let result = provider.parse_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":\"Tok"}}"#,
        ).unwrap();
        let chunk = result.unwrap();
        let tool_calls = chunk.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].index, 1);
        assert_eq!(
            tool_calls[0].function.as_ref().unwrap().arguments,
            Some("{\"location\":\"Tok".to_string())
        );
    }

    #[test]
    fn test_parse_content_block_stop() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: content_block_stop")
            .unwrap();
        let result = provider
            .parse_anthropic_stream_line(r#"data: {"type":"content_block_stop","index":0}"#)
            .unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
        assert!(chunk.finish_reason.is_none());
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn build_body_concatenates_multiple_system_messages() {
        let provider = make_provider();
        let request = LlmRequest {
            messages: vec![
                agent_types::ChatMessage::system("base system"),
                agent_types::ChatMessage::system("workspace rules"),
                agent_types::ChatMessage::user("hello"),
            ],
            tools: Vec::new(),
            tool_choice: agent_types::ToolChoice::Auto,
            max_tokens: None,
            temperature: None,
            response_format: agent_types::ResponseFormat::Text,
            reasoning_effort: agent_types::ReasoningEffort::Off,
        };

        let body = provider.build_body(&request, false);

        assert_eq!(body["system"], "base system\n\nworkspace rules");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_body_sets_thinking_budget_for_reasoning_effort() {
        let provider = make_provider();
        let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
        request.max_tokens = Some(4096);
        request.reasoning_effort = agent_types::ReasoningEffort::Max;

        let body = provider.build_body(&request, false);

        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 2048);
    }

    #[test]
    fn build_body_omits_thinking_when_reasoning_effort_is_off() {
        let provider = make_provider();
        let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

        let body = provider.build_body(&request, false);

        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn test_parse_unknown_event() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: unknown_event")
            .unwrap();
        let result = provider
            .parse_anthropic_stream_line(r#"data: {"type":"unknown_event"}"#)
            .unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
    }

    #[test]
    fn test_parse_malformed_data() {
        let provider = make_provider();
        provider
            .parse_anthropic_stream_line("event: content_block_delta")
            .unwrap();
        let result = provider
            .parse_anthropic_stream_line("data: invalid json")
            .unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
    }

    #[test]
    fn test_parse_empty_line() {
        let provider = make_provider();
        let result = provider.parse_anthropic_stream_line("").unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
    }

    #[test]
    fn test_multiple_content_deltas() {
        let provider = make_provider();

        provider
            .parse_anthropic_stream_line("event: content_block_delta")
            .unwrap();
        let result = provider.parse_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        ).unwrap();
        assert_eq!(result.unwrap().content, Some("Hello".to_string()));

        provider
            .parse_anthropic_stream_line("event: content_block_delta")
            .unwrap();
        let result = provider.parse_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" World"}}"#,
        ).unwrap();
        assert_eq!(result.unwrap().content, Some(" World".to_string()));
    }
}
