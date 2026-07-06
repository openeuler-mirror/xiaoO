use crate::LlmRequest;

pub struct PromptBuildResult {
    pub request: LlmRequest,
    pub estimated_input_tokens: usize,
    /// The ordered system prompt parts the builder composed before joining
    /// them into `request.messages[0]`. Exposed so a downstream stage (the
    /// agent loop) can run the `*.Chat.system.transform` hooker on the
    /// un-merged array — mirroring opencode's `experimental.chat.system.transform`.
    pub system_parts: Vec<String>,
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
