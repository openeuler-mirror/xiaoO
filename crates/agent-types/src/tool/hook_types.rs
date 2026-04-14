use crate::tool::call_types::FinalToolCall;
use crate::tool::execution_types::{RawToolOutcome, ToolExecutionError};

#[derive(Clone, Debug)]
pub struct PreToolHookInput {
    pub call: FinalToolCall,
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
