use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoopEndSummary {
    pub turn_count: u32,
    pub total_tokens: usize,
    pub stop_reason: String,
}
