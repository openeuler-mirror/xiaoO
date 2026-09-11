use serde::{Deserialize, Serialize};

use super::response::{KvTransferParams, WireUsage};
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
    #[serde(default)]
    pub kv_transfer_params: Option<KvTransferParams>,
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
    #[serde(alias = "reasoning_content")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedChunk {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub finish_reason: Option<String>,
    pub usage: Option<WireUsage>,
    pub tool_calls: Option<Vec<WireToolCallDelta>>,
    pub kv_transfer_params: Option<KvTransferParams>,
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/wire_types/stream_test.rs"]
mod tests;
