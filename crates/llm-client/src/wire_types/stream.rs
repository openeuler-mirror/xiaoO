use serde::{Deserialize, Serialize};

use super::response::WireUsage;
use super::tool::WireToolCallDelta;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedChunk {
    pub content: Option<String>,
    #[allow(dead_code)]
    pub reasoning: Option<String>,
    pub finish_reason: Option<String>,
    pub usage: Option<WireUsage>,
    pub tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsed_chunk() {
        let chunk = ParsedChunk::default();
        assert!(chunk.content.is_none());
        assert!(chunk.finish_reason.is_none());
        assert!(chunk.usage.is_none());

        let usage = WireUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        };
        let chunk = ParsedChunk {
            content: Some("Hello".to_string()),
            reasoning: None,
            finish_reason: Some("stop".to_string()),
            usage: Some(usage),
            tool_calls: None,
        };
        assert_eq!(chunk.content, Some("Hello".to_string()));
        assert_eq!(chunk.finish_reason, Some("stop".to_string()));
        assert_eq!(chunk.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn test_parsed_chunk_with_tool_calls() {
        let tool_call_delta = WireToolCallDelta {
            index: 0,
            id: Some("call_123".to_string()),
            call_type: Some("function".to_string()),
            function: Some(super::super::tool::WireToolCallFunctionDelta {
                name: Some("test".to_string()),
                arguments: Some("{\"a\":".to_string()),
            }),
        };

        let chunk = ParsedChunk {
            content: Some("Hello".to_string()),
            reasoning: None,
            finish_reason: None,
            usage: None,
            tool_calls: Some(vec![tool_call_delta]),
        };

        assert!(chunk.tool_calls.is_some());
        let deltas = chunk.tool_calls.as_ref().unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].index, 0);
    }
}
