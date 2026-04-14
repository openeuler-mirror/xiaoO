use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use super::openai_family::parse_openai_family_stream_line;
use crate::convert::{chat_messages_to_wire, parsed_chunk_to_stream_chunk, wire_usage_to_usage};
use crate::error::{map_reqwest_error, LlmError};
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
            let resp_body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "HTTP {}: {}\nRequest body: {}",
                status, resp_body, body_str
            )));
        }

        let ollama_response: serde_json::Value =
            response.json().await.map_err(map_reqwest_error)?;
        let content = ollama_response["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some(content),
                tool_calls: vec![],
                usage: Usage::default(),
                stop_reason: StopReason::EndTurn,
            },
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
            let error_body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "HTTP {}: {}\nRequest body: {}",
                status, error_body, body_str
            )));
        }

        let mut full_text = String::new();
        let mut final_usage = None;

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

                if let Some(parsed) = parse_openai_family_stream_line(&line)? {
                    if let Some(ref content) = parsed.content {
                        full_text.push_str(content);
                    }
                    if let Some(ref usage) = parsed.usage {
                        final_usage = Some(wire_usage_to_usage(usage));
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
                tool_calls: vec![],
                usage: final_usage.unwrap_or_default(),
                stop_reason: StopReason::EndTurn,
            },
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}
