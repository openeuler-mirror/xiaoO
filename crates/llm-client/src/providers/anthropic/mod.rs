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
    anthropic_messages, anthropic_system_blocks, extract_anthropic_tool_calls,
    to_anthropic_output_format, to_anthropic_tool, to_anthropic_tool_choice,
};

pub(crate) struct AnthropicProvider {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
    capabilities: ProviderCapabilities,
    api_key_provider: Option<crate::factory::ApiKeyProviderFn>,
}

impl AnthropicProvider {
    pub(crate) fn new(
        api_key: Option<String>,
        base_url: String,
        model: String,
        api_key_provider: Option<crate::factory::ApiKeyProviderFn>,
    ) -> Self {
        let max_context_window = crate::models::get_known_model_context_length(&model)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(200000);
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
                max_context_window,
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

    fn build_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        let system_blocks = anthropic_system_blocks(&request.messages);
        let mut other_messages = anthropic_messages(&request.messages);

        // Incremental prefix caching over the conversation history: mark the
        // final content block of the last two non-system messages as cache
        // breakpoints. The prompt builder appends the per-turn `<system-reminder>`
        // context as the final message (ephemeral, never persisted), so the
        // breakpoint on the second-to-last message is the one that lands on
        // stable history and yields the cache hit next turn; the one on the
        // tail costs only a small re-write of the reminder itself. Budget:
        // 2 breakpoints here + system breakpoints stays within Anthropic's
        // limit of 4.
        for msg in other_messages.iter_mut().rev().take(2) {
            if let Some(last_block) = msg["content"]
                .as_array_mut()
                .and_then(|blocks| blocks.last_mut())
            {
                last_block["cache_control"] = serde_json::json!({ "type": "ephemeral" });
            }
        }

        let requested_max_tokens = request.max_tokens.unwrap_or(16384);
        let (max_tokens, thinking, output_effort) = anthropic_reasoning_config(
            &self.capabilities.model_name,
            request.reasoning_effort,
            requested_max_tokens,
        );

        let mut body = serde_json::json!({
            "model": self.capabilities.model_name,
            "messages": other_messages,
            "max_tokens": max_tokens,
        });

        if stream {
            body["stream"] = serde_json::json!(true);
        }

        if !system_blocks.is_empty() {
            let block_count = system_blocks.len();
            let blocks: Vec<serde_json::Value> = system_blocks
                .into_iter()
                .enumerate()
                .map(|(idx, text)| {
                    let mut block = serde_json::json!({ "type": "text", "text": text });
                    if block_count == 1 || idx + 1 < block_count {
                        block["cache_control"] = serde_json::json!({ "type": "ephemeral" });
                    }
                    block
                })
                .collect();
            body["system"] = serde_json::json!(blocks);
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

        let mut output_config = serde_json::Map::new();
        let wire_format = crate::convert::response_format_to_wire(&request.response_format);
        if let Some(ref wf) = wire_format {
            if let Some(output_format) = to_anthropic_output_format(wf) {
                output_config.insert("format".to_string(), output_format);
            }
        }

        if let Some(effort) = output_effort {
            output_config.insert("effort".to_string(), serde_json::json!(effort));
        }
        if !output_config.is_empty() {
            body["output_config"] = serde_json::Value::Object(output_config);
        }

        if let Some(thinking) = thinking {
            body["thinking"] = thinking;
        }

        body
    }
}

fn anthropic_reasoning_config(
    model: &str,
    effort: ReasoningEffort,
    requested_max_tokens: usize,
) -> (usize, Option<serde_json::Value>, Option<&'static str>) {
    if is_claude_fable_5_model(model) || is_claude_sonnet_5_model(model) {
        return match effort {
            ReasoningEffort::Off if is_claude_sonnet_5_model(model) => (
                requested_max_tokens,
                Some(serde_json::json!({ "type": "disabled" })),
                None,
            ),
            // Fable 5 always uses adaptive thinking, so its "off" setting is
            // represented by omitting an unsupported disabled request.
            ReasoningEffort::Off => (requested_max_tokens, None, None),
            ReasoningEffort::High => (
                requested_max_tokens,
                Some(serde_json::json!({ "type": "adaptive" })),
                Some("high"),
            ),
            ReasoningEffort::Max => (
                requested_max_tokens,
                Some(serde_json::json!({ "type": "adaptive" })),
                Some("max"),
            ),
        };
    }

    let (max_tokens, thinking_budget) = anthropic_reasoning_budget(effort, requested_max_tokens);
    let thinking = thinking_budget.map(|budget_tokens| {
        serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget_tokens,
        })
    });
    (max_tokens, thinking, None)
}

fn is_claude_fable_5_model(model: &str) -> bool {
    normalized_model_leaf(model).starts_with("claude-fable-5")
}

fn is_claude_sonnet_5_model(model: &str) -> bool {
    normalized_model_leaf(model).starts_with("claude-sonnet-5")
}

fn normalized_model_leaf(model: &str) -> String {
    model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
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

fn anthropic_stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "refusal" => StopReason::ContentFilter,
        _ => StopReason::EndTurn,
    }
}

fn anthropic_response_text(response: &serde_json::Value) -> Option<String> {
    let content = response["content"].as_array().map(|blocks| {
        blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("")
    });

    content.filter(|text| !text.is_empty()).or_else(|| {
        response["stop_details"]["explanation"]
            .as_str()
            .filter(|explanation| !explanation.is_empty())
            .map(str::to_string)
    })
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
            .header("x-api-key", self.get_api_key())
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

        let content = anthropic_response_text(&anthropic_response);
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
            cached_tokens: usage_val["cache_read_input_tokens"].as_u64().unwrap_or(0) as usize,
        };

        let finish_reason = anthropic_response["stop_reason"]
            .as_str()
            .unwrap_or("end_turn");
        let stop_reason = anthropic_stop_reason(finish_reason);

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
                    content
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
            .header("x-api-key", self.get_api_key())
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
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
        // Per-stream event_type state. Scoped locally so concurrent streams
        // on the same shared provider don't race on a shared Mutex.
        let mut current_event: Option<String> = None;

        while let Some(chunk_result) = byte_stream.next().await {
            let bytes = chunk_result.map_err(|e| {
                crate::error::write_stream_error_log(
                    &url,
                    Some(&headers),
                    &buffer,
                    &e.to_string(),
                    Some(status.as_u16()),
                );
                LlmError::StreamError {
                    message: format!("{} (详见 ~/.xiaoo/log/error.log)", e),
                }
            })?;
            let text = String::from_utf8_lossy(&bytes);
            buffer.push_str(&text);

            // Cursor-based SSE scanning: find each '\n' without rebuilding
            // the buffer per line; drain the consumed prefix in place.
            let mut start = 0;
            while let Some(relative) = buffer[start..].find('\n') {
                let pos = start + relative;
                let line = &buffer[start..pos];
                start = pos + 1;

                if line.is_empty() {
                    continue;
                }

                if let Some(parsed) = Self::parse_anthropic_stream_line(&mut current_event, line)? {
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
                        final_stop_reason = anthropic_stop_reason(reason);
                    }
                    super::openai_family::accumulate_tool_call_deltas_pub(
                        &mut full_tool_calls,
                        &parsed,
                    );

                    let stream_chunk = parsed_chunk_to_stream_chunk(&parsed);
                    on_chunk(stream_chunk);
                }
            }
            // Drop the consumed prefix in place; no per-line allocation.
            if start > 0 {
                buffer.drain(..start);
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
    /// Parses one SSE line, using `current_event` to remember the last
    /// `event:` line so subsequent `data:` lines can dispatch on the event
    /// type. State is caller-supplied (a local in `complete_stream`) so
    /// concurrent streams on the same shared provider don't race.
    fn parse_anthropic_stream_line(
        current_event: &mut Option<String>,
        line: &str,
    ) -> Result<Option<ParsedChunk>, LlmError> {
        if let Some(event_type) = line.strip_prefix("event: ") {
            *current_event = Some(event_type.to_string());
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

        // Clone the event_type out to release the borrow on `current_event`
        // before the match below.
        let event_type = current_event.clone();

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
                    prompt_tokens_details: None,
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
                    prompt_tokens_details: None,
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
    merged.cached_tokens = merged.cached_tokens.max(incoming.cached_tokens);
    merged.total_tokens = merged.prompt_tokens + merged.completion_tokens;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::{ChatMessageExt, LlmRequestExt};
    use agent_types::LlmRequest;

    fn make_provider() -> AnthropicProvider {
        make_provider_for_model("claude-sonnet-4-6")
    }

    fn make_provider_for_model(model: &str) -> AnthropicProvider {
        AnthropicProvider::new(
            Some("test-key".to_string()),
            "https://api.anthropic.com/v1".to_string(),
            model.to_string(),
            None,
        )
    }

    #[test]
    fn test_parse_content_block_delta() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "event: content_block_delta",
        )
        .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().content, Some("Hello".to_string()));
    }

    #[test]
    fn test_parse_message_delta() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: message_delta")
            .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#,
        )
        .unwrap();
        let chunk = result.unwrap();
        assert_eq!(chunk.finish_reason, Some("end_turn".to_string()));
        assert!(chunk.usage.is_some());
        assert_eq!(chunk.usage.unwrap().completion_tokens, 15);
    }

    #[test]
    fn test_parse_message_start_usage() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: message_start")
            .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
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
                cached_tokens: 0,
            }),
            Usage {
                prompt_tokens: 0,
                completion_tokens: 15,
                total_tokens: 15,
                cached_tokens: 0,
            },
        );

        assert_eq!(merged.prompt_tokens, 21);
        assert_eq!(merged.completion_tokens, 15);
        assert_eq!(merged.total_tokens, 36);
    }

    #[test]
    fn test_parse_message_stop() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: message_stop")
            .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"message_stop"}"#,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_input_json_delta_as_tool_call() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "event: content_block_delta",
        )
        .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":\"Tok"}}"#,
        )
        .unwrap();
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
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "event: content_block_stop",
        )
        .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"content_block_stop","index":0}"#,
        )
        .unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
        assert!(chunk.finish_reason.is_none());
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn build_body_emits_system_blocks_caching_only_the_stable_prefix() {
        let provider = make_provider();
        let request = LlmRequest {
            messages: vec![
                agent_types::ChatMessage::system("base system"),
                agent_types::ChatMessage::system("# Context\n\nvolatile tail"),
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

        let system = body["system"]
            .as_array()
            .expect("system should be an array");
        assert_eq!(system.len(), 2);
        // Stable prefix carries the cache breakpoint; the volatile tail does not,
        // so a per-turn change to the tail never invalidates the cached prefix.
        assert_eq!(system[0]["text"], "base system");
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(system[1]["text"], "# Context\n\nvolatile tail");
        assert!(system[1].get("cache_control").is_none());
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_body_marks_last_two_messages_as_cache_breakpoints() {
        let provider = make_provider();
        let request = LlmRequest {
            messages: vec![
                agent_types::ChatMessage::system("base system"),
                agent_types::ChatMessage::user("first question"),
                agent_types::ChatMessage::assistant("first answer", 0),
                agent_types::ChatMessage::user("second question"),
                agent_types::ChatMessage::user(
                    "<system-reminder>\n# Context\nper-turn state\n</system-reminder>",
                ),
            ],
            tools: Vec::new(),
            tool_choice: agent_types::ToolChoice::Auto,
            max_tokens: None,
            temperature: None,
            response_format: agent_types::ResponseFormat::Text,
            reasoning_effort: agent_types::ReasoningEffort::Off,
        };

        let body = provider.build_body(&request, false);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);

        // Older history carries no breakpoint.
        assert!(messages[0]["content"][0].get("cache_control").is_none());
        assert!(messages[1]["content"][0].get("cache_control").is_none());
        // The last two messages do: the second-to-last lands on stable
        // history (cache hit next turn); the last covers the ephemeral
        // per-turn reminder tail.
        assert_eq!(
            messages[2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            messages[3]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn build_body_caches_single_system_block() {
        let provider = make_provider();
        let request = LlmRequest {
            messages: vec![
                agent_types::ChatMessage::system("base system only"),
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

        let system = body["system"]
            .as_array()
            .expect("system should be an array");
        assert_eq!(system.len(), 1);
        assert_eq!(system[0]["text"], "base system only");
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
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
    fn claude_5_capability_uses_known_context_window() {
        let provider = make_provider_for_model("claude-sonnet-5");

        assert_eq!(provider.capabilities.max_context_window, 1_000_000);
    }

    #[test]
    fn sonnet_5_disables_thinking_when_reasoning_effort_is_off() {
        let provider = make_provider_for_model("claude-sonnet-5");
        let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

        let body = provider.build_body(&request, false);

        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn fable_5_does_not_request_unsupported_disabled_thinking() {
        let provider = make_provider_for_model("claude-fable-5");
        let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

        let body = provider.build_body(&request, false);

        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn claude_5_uses_adaptive_thinking_and_model_effort() {
        let provider = make_provider_for_model("anthropic/claude-sonnet-5");
        let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
        request.reasoning_effort = ReasoningEffort::Max;

        let body = provider.build_body(&request, false);

        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "max");
        assert!(body["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn claude_5_merges_structured_output_format_with_effort() {
        let provider = make_provider_for_model("claude-fable-5");
        let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
        request.reasoning_effort = ReasoningEffort::High;
        request.response_format = agent_types::ResponseFormat::JsonSchema {
            name: "answer".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "answer": { "type": "string" } }
            }),
        };

        let body = provider.build_body(&request, false);

        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    }

    #[test]
    fn refusal_maps_to_content_filter_and_uses_explanation_text() {
        assert!(matches!(
            anthropic_stop_reason("refusal"),
            StopReason::ContentFilter
        ));
        let response = serde_json::json!({
            "content": [],
            "stop_reason": "refusal",
            "stop_details": { "explanation": "I can't help with that request." }
        });

        assert_eq!(
            anthropic_response_text(&response).as_deref(),
            Some("I can't help with that request.")
        );
    }

    #[test]
    fn test_parse_unknown_event() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: unknown_event")
            .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"unknown_event"}"#,
        )
        .unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
    }

    #[test]
    fn test_parse_malformed_data() {
        let mut current_event = None;
        AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "event: content_block_delta",
        )
        .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "data: invalid json",
        )
        .unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
    }

    #[test]
    fn test_parse_empty_line() {
        let mut current_event = None;
        let result =
            AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "").unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.content.is_none());
    }

    #[test]
    fn test_multiple_content_deltas() {
        let mut current_event = None;

        AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "event: content_block_delta",
        )
        .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .unwrap();
        assert_eq!(result.unwrap().content, Some("Hello".to_string()));

        AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            "event: content_block_delta",
        )
        .unwrap();
        let result = AnthropicProvider::parse_anthropic_stream_line(
            &mut current_event,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" World"}}"#,
        )
        .unwrap();
        assert_eq!(result.unwrap().content, Some(" World".to_string()));
    }
}
