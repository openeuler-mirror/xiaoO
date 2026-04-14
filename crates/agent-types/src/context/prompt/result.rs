use crate::LlmRequest;

pub struct PromptBuildResult {
    pub request: LlmRequest,
    pub estimated_input_tokens: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PromptBuildError {
    #[error("system prompt too large: {tokens} tokens exceeds budget")]
    SystemPromptTooLarge { tokens: usize },

    #[error("no messages to build prompt from")]
    EmptyMessages,

    #[error("build failed: {message}")]
    BuildFailed { message: String },
}
