use super::CompressionMeta;
use crate::llm::ChatMessage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct CompressedView {
    pub messages: Vec<ChatMessage>,
    pub removed_count: usize,
    pub summary: Option<String>,
    pub updated_meta: CompressionMeta,
    pub estimated_tokens: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MicroCompactResult {
    pub applied: bool,
    pub removed_count: usize,
    pub removed_call_ids: Vec<String>,
    pub messages: Vec<ChatMessage>,
    pub token_delta: isize,
}
