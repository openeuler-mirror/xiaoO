use crate::llm::ChatMessage;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

pub enum AgentOutcome {
    Complete {
        reply: String,
        messages: Vec<ChatMessage>,
        turn_count: u32,
        token_usage: TokenUsage,
        estimated_input_tokens: usize,
    },

    MaxTurnsReached {
        partial_reply: Option<String>,
        messages: Vec<ChatMessage>,
        turn_count: u32,
        token_usage: TokenUsage,
        estimated_input_tokens: usize,
    },

    BudgetExhausted {
        partial_reply: Option<String>,
        messages: Vec<ChatMessage>,
        turn_count: u32,
        token_usage: TokenUsage,
        estimated_input_tokens: usize,
    },

    Cancelled {
        partial_reply: Option<String>,
        messages: Vec<ChatMessage>,
        turn_count: u32,
        token_usage: TokenUsage,
        estimated_input_tokens: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM provider error: {0}")]
    LlmProvider(String),

    #[error("tool execution error: {0}")]
    ToolExecution(String),

    #[error("compression error: {0}")]
    Compression(String),

    #[error("prompt build error: {0}")]
    PromptBuild(String),

    #[error("token budget exhausted")]
    TokenBudgetExhausted,
}
