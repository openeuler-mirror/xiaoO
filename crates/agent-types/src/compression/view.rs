use super::CompressionMeta;
use crate::llm::ChatMessage;

#[derive(Clone, Debug)]
pub struct CompressedView {
    pub messages: Vec<ChatMessage>,
    pub removed_count: usize,
    pub summary: Option<String>,
    pub updated_meta: CompressionMeta,
    pub estimated_tokens: usize,
}
