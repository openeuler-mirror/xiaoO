use crate::session::hook_types::{
    SessionClosedHookInput, SessionCreatedHookInput, SessionHookResult,
};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookInvokeMetadata {
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum HookInvokeInput {
    // Tool hook variants
    Pre {
        input: PreToolHookInput,
        metadata: HookInvokeMetadata,
    },
    Post {
        input: PostToolHookInput,
        metadata: HookInvokeMetadata,
    },
    Error {
        input: ErrorToolHookInput,
        metadata: HookInvokeMetadata,
    },
    // LLM hook variants
    LlmPre {
        input: PreLlmHookInput,
        metadata: HookInvokeMetadata,
    },
    LlmPost {
        input: PostLlmHookInput,
        metadata: HookInvokeMetadata,
    },
    LlmError {
        input: ErrorLlmHookInput,
        metadata: HookInvokeMetadata,
    },
    // Session hook variants
    SessionCreated {
        input: SessionCreatedHookInput,
        metadata: HookInvokeMetadata,
    },
    SessionClosed {
        input: SessionClosedHookInput,
        metadata: HookInvokeMetadata,
    },
}

impl HookInvokeInput {
    pub fn metadata(&self) -> &HookInvokeMetadata {
        match self {
            Self::Pre { metadata, .. }
            | Self::Post { metadata, .. }
            | Self::Error { metadata, .. }
            | Self::LlmPre { metadata, .. }
            | Self::LlmPost { metadata, .. }
            | Self::LlmError { metadata, .. }
            | Self::SessionCreated { metadata, .. }
            | Self::SessionClosed { metadata, .. } => metadata,
        }
    }
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
    // Session hook variants
    SessionCreated(SessionHookResult),
    SessionClosed(SessionHookResult),
}
