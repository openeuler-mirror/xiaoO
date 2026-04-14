use crate::llm::error::LlmError;
use crate::llm::hook_types::{
    ErrorLlmHookInput, ErrorLlmHookResult, PostLlmHookInput, PostLlmHookResult, PreLlmHookInput,
    PreLlmHookResult,
};
use crate::tool::execution_types::ToolExecutionError;
use crate::tool::hook_types::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput,
};

#[derive(Debug, thiserror::Error)]
pub enum HookInvokeError {
    #[error("{0}")]
    Tool(#[from] ToolExecutionError),
    #[error("{0}")]
    Llm(#[from] LlmError),
}

#[derive(Clone, Debug)]
pub enum HookInvokeInput {
    // Tool hook variants
    Pre(PreToolHookInput),
    Post(PostToolHookInput),
    Error(ErrorToolHookInput),
    // LLM hook variants
    LlmPre(PreLlmHookInput),
    LlmPost(PostLlmHookInput),
    LlmError(ErrorLlmHookInput),
}

#[derive(Clone, Debug)]
pub enum HookInvokeOutput {
    // Tool hook variants
    Pre(PreHookResult),
    Post(PostHookResult),
    Error(ErrorHookResult),
    // LLM hook variants
    LlmPre(PreLlmHookResult),
    LlmPost(PostLlmHookResult),
    LlmError(ErrorLlmHookResult),
}
