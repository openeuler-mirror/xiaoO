use crate::tool::call_types::FinalToolCall;
use crate::tool::hook_types::{ErrorHookResult, PostHookResult, PreHookResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RawToolOutcome {
    Success { output: String },
    Error { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolExecutorOutput {
    Completed { raw_outcome: RawToolOutcome },
    Suspended { suspend_token: String },
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ToolExecutionError {
    #[error("tool not found: {tool_name}")]
    NotFound { tool_name: String },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("permission denied: {message}")]
    PermissionDenied { message: String },
}

#[derive(Clone, Debug)]
pub enum ToolExecutionResult {
    Denied {
        final_call: FinalToolCall,
        pre_hook_results: Vec<PreHookResult>,
        error_hook_results: Vec<ErrorHookResult>,
        error: Option<ToolExecutionError>,
    },
    Completed {
        final_call: FinalToolCall,
        raw_outcome: RawToolOutcome,
        pre_hook_results: Vec<PreHookResult>,
        post_hook_results: Vec<PostHookResult>,
    },
    Suspended {
        final_call: FinalToolCall,
        pre_hook_results: Vec<PreHookResult>,
        suspend_token: String,
    },
    Failed {
        final_call: FinalToolCall,
        pre_hook_results: Vec<PreHookResult>,
        error_hook_results: Vec<ErrorHookResult>,
        execution_error: ToolExecutionError,
    },
}

impl ToolExecutionResult {
    pub fn final_call(&self) -> &FinalToolCall {
        match self {
            Self::Denied { final_call, .. } => final_call,
            Self::Completed { final_call, .. } => final_call,
            Self::Suspended { final_call, .. } => final_call,
            Self::Failed { final_call, .. } => final_call,
        }
    }

    pub fn call_id(&self) -> &str {
        &self.final_call().call_id
    }

    pub fn tool_name(&self) -> &str {
        &self.final_call().tool_name
    }
}
