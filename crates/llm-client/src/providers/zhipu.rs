use async_trait::async_trait;

use crate::error::LlmError;
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_llm::ChatMessageExt;
use agent_types::{ChatMessage, LlmRequest, LlmResponse, MessageRole, ResponseFormat, StreamChunk};

use super::openai_family::{OpenAiFamilyAuthStyle, OpenAiFamilyProvider};

pub(crate) struct ZhipuProvider {
    inner: OpenAiFamilyProvider,
}

impl ZhipuProvider {
    pub(crate) fn new(api_key: String, api_base: String, model: String) -> Self {
        Self {
            inner: OpenAiFamilyProvider::new(
                api_key,
                api_base,
                model,
                OpenAiFamilyAuthStyle::Bearer,
                vec![],
            ),
        }
    }

    fn schema_to_format_description(schema: &serde_json::Value) -> String {
        match schema {
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|t| t.as_str()) == Some("object") {
                    if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
                        let fields: Vec<String> = props
                            .iter()
                            .map(|(key, value)| {
                                let type_str =
                                    value.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                                let desc = value
                                    .get("description")
                                    .and_then(|d| d.as_str())
                                    .map(|d| format!(" // {}", d))
                                    .unwrap_or_default();
                                format!("  \"{}\": {}{}", key, type_str, desc)
                            })
                            .collect();

                        let required = map
                            .get("required")
                            .and_then(|r| r.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());

                        let required_note = required
                            .map(|r| format!("\nRequired fields: {}", r.join(", ")))
                            .unwrap_or_default();

                        return format!("{{\n{}\n}}{}", fields.join(",\n"), required_note);
                    }
                }
                schema.to_string()
            }
            _ => schema.to_string(),
        }
    }

    /// Zhipu doesn't support json_schema response_format natively.
    /// Inject schema into system message and downgrade to json_object.
    fn transform_request(request: &LlmRequest) -> LlmRequest {
        match &request.response_format {
            ResponseFormat::JsonSchema { name: _, schema } => {
                let format_desc = Self::schema_to_format_description(schema);
                let schema_message = format!(
                    "You must respond in valid JSON format matching this structure:\n{}",
                    format_desc
                );

                let mut new_messages = request.messages.clone();
                let insert_pos = new_messages
                    .iter()
                    .rposition(|m| m.role == MessageRole::System)
                    .map(|i| i + 1)
                    .unwrap_or(0);

                new_messages.insert(insert_pos, ChatMessage::system(schema_message));

                LlmRequest {
                    messages: new_messages,
                    tools: request.tools.clone(),
                    tool_choice: request.tool_choice.clone(),
                    max_tokens: request.max_tokens,
                    temperature: request.temperature,
                    response_format: ResponseFormat::JsonObject,
                    reasoning_effort: request.reasoning_effort,
                }
            }
            _ => request.clone(),
        }
    }
}

#[async_trait]
impl LlmProvider for ZhipuProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let transformed = Self::transform_request(request);
        self.inner.complete(&transformed).await
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let transformed = Self::transform_request(request);
        self.inner.complete_stream(&transformed, on_chunk).await
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.inner.capabilities()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_to_format_description_simple() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });
        let desc = ZhipuProvider::schema_to_format_description(&schema);
        assert!(desc.contains("\"name\": string"));
        assert!(desc.contains("\"age\": integer"));
        assert!(desc.contains("Required fields: name, age"));
    }

    #[test]
    fn test_schema_to_format_description_with_description() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The person's name"},
                "age": {"type": "integer", "description": "The person's age"}
            }
        });
        let desc = ZhipuProvider::schema_to_format_description(&schema);
        assert!(desc.contains("// The person's name"));
        assert!(desc.contains("// The person's age"));
    }
}
