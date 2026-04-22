use crate::tool::call_types::FinalToolCall;
use crate::tool::execution_types::{RawToolOutcome, ToolExecutionError};
use crate::ChatMessage;

#[derive(Clone, Debug)]
pub struct PreToolHookInput {
    pub call: FinalToolCall,
    /// Recent messages from conversation, used for extracting action history
    /// (completed tool calls) for security rules like read_before_write.
    pub recent_messages: Vec<ChatMessage>,
}

#[derive(Clone, Debug)]
pub enum PreHookResult {
    Allow,
    Deny { reason: String },
    Transform { modified_input: serde_json::Value },
}

#[derive(Clone, Debug)]
pub struct PostToolHookInput {
    pub call: FinalToolCall,
    pub outcome: RawToolOutcome,
}

#[derive(Clone, Debug)]
pub enum PostHookResult {
    Accept,
    Transform { modified_output: String },
}

#[derive(Clone, Debug)]
pub struct ErrorToolHookInput {
    pub call: FinalToolCall,
    pub error: ToolExecutionError,
}

#[derive(Clone, Debug)]
pub enum ErrorHookResult {
    Propagate,
    Recover { output: String },
}
