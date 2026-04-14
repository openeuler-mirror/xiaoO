use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstructionMemory {
    pub source: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactMemory {
    pub key: String,
    pub content: String,
    pub recorded_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskMemory {
    pub current_task: String,
    pub pending_steps: Vec<String>,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptHistoryEntry {
    pub prompt: String,
    pub recorded_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsageBaseline {
    pub estimated_history_tokens: usize,
    pub last_prompt_tokens: usize,
    pub last_completion_tokens: usize,
    pub recorded_at: u64,
}
