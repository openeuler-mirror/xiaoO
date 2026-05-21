use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use super::openai_family::parse_openai_family_stream_line;
use crate::convert::{chat_messages_to_wire, parsed_chunk_to_stream_chunk, wire_usage_to_usage};
use crate::error::{map_api_status_error, map_reqwest_error, map_serde_error, LlmError};
use crate::wire_types::{ParsedChunk, WireUsage};
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::{AssistantMessage, LlmRequest, LlmResponse, StopReason, StreamChunk, Usage};

pub(crate) struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    capabilities: ProviderCapabilities,
}

impl OllamaProvider {
    pub(crate) fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            base_url,
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: true,
                supports_json_mode: true,
                max_context_window: 128000,
                model_name: model,
            },
        }
    }

    fn build_ollama_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        let wire_messages = chat_messages_to_wire(&request.messages);

        let mut body = serde_json::json!({
            "model": self.capabilities.model_name,
            "messages": wire_messages,
            "stream": stream,
        });

        let wire_format = crate::convert::response_format_to_wire(&request.response_format);
        if let Some(ref wf) = wire_format {
            match wf.format_type.as_str() {
                "json_object" => {
                    body["format"] = serde_json::json!("json");
                }
                "json_schema" => {
                    if let Some(ref schema_def) = wf.json_schema {
                        body["format"] = schema_def.schema.clone();
                    } else {
                        body["format"] = serde_json::json!("json");
                    }
                }
                _ => {}
            }
        }

        if !request.tools.is_empty() {
            let wire_tools: Vec<_> = request
                .tools
                .iter()
                .map(|t| crate::convert::tool_to_wire(t))
                .collect();
            body["tools"] = serde_json::json!(wire_tools);
        }

        body
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let url = format!("{}/api/chat", self.base_url);
        let body = self.build_ollama_body(request, false);
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

        let ollama_response: serde_json::Value =
            response.json().await.map_err(map_reqwest_error)?;
        let content = ollama_response["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let usage = usage_from_ollama_json(&ollama_response);
        let stop_reason = stop_reason_from_ollama_json(&ollama_response);

        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some(content),
                reasoning_content: None,
                tool_calls: vec![],
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
        let url = format!("{}/api/chat", self.base_url);
        let body = self.build_ollama_body(request, true);
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

                if let Some(parsed) = parse_ollama_stream_line(&line)? {
                    if let Some(ref content) = parsed.content {
                        full_text.push_str(content);
                    }
                    if let Some(ref usage) = parsed.usage {
                        final_usage = Some(wire_usage_to_usage(usage));
                    }
                    if let Some(ref reason) = parsed.finish_reason {
                        final_stop_reason = match reason.as_str() {
                            "stop" | "end_turn" => StopReason::EndTurn,
                            "length" | "max_tokens" => StopReason::MaxTokens,
                            _ => StopReason::EndTurn,
                        };
                    }
                    on_chunk(parsed_chunk_to_stream_chunk(&parsed));
                }
            }
        }

        Ok(LlmResponse {
            message: AssistantMessage {
                text: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                reasoning_content: None,
                tool_calls: vec![],
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

fn usage_from_ollama_json(json: &serde_json::Value) -> Usage {
    let prompt_tokens = json["prompt_eval_count"].as_u64().unwrap_or(0) as usize;
    let completion_tokens = json["eval_count"].as_u64().unwrap_or(0) as usize;
    Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

fn stop_reason_from_ollama_json(json: &serde_json::Value) -> StopReason {
    match json["done_reason"].as_str() {
        Some("length") => StopReason::MaxTokens,
        Some("stop") | Some("end_turn") | None => StopReason::EndTurn,
        _ => StopReason::EndTurn,
    }
}

fn parse_ollama_stream_line(line: &str) -> Result<Option<ParsedChunk>, LlmError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Some(ParsedChunk::default()));
    }

    if line.starts_with("data:") || line == "[DONE]" {
        return parse_openai_family_stream_line(line);
    }

    let json: serde_json::Value = serde_json::from_str(line).map_err(map_serde_error)?;
    let content = json["message"]["content"].as_str().map(|s| s.to_string());
    let usage = {
        let usage = usage_from_ollama_json(&json);
        if usage.total_tokens == 0 {
            None
        } else {
            Some(WireUsage {
                prompt_tokens: usage.prompt_tokens as u32,
                completion_tokens: usage.completion_tokens as u32,
                total_tokens: usage.total_tokens as u32,
            })
        }
    };
    let finish_reason = json["done"]
        .as_bool()
        .filter(|done| *done)
        .map(|_| json["done_reason"].as_str().unwrap_or("stop").to_string());

    Ok(Some(ParsedChunk {
        content,
        reasoning: None,
        finish_reason,
        usage,
        tool_calls: None,
        kv_transfer_params: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_ollama_json_maps_eval_counts() {
        let json = serde_json::json!({
            "prompt_eval_count": 32,
            "eval_count": 18,
        });

        let usage = usage_from_ollama_json(&json);

        assert_eq!(usage.prompt_tokens, 32);
        assert_eq!(usage.completion_tokens, 18);
        assert_eq!(usage.total_tokens, 50);
    }

    #[test]
    fn parse_ollama_stream_line_extracts_content_and_usage() {
        let parsed = parse_ollama_stream_line(
            r#"{"message":{"content":"hello"},"done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":7}"#,
        )
        .expect("ollama stream line should parse")
        .expect("parsed chunk expected");

        assert_eq!(parsed.content, Some("hello".to_string()));
        assert_eq!(parsed.finish_reason, Some("stop".to_string()));
        let usage = parsed.usage.expect("usage should be extracted");
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 19);
    }
}
