use crate::chat::{
    ChatHookError, ChatMessageHookInput, ChatMessageHookResult, ChatSystemTransformInput,
    ChatSystemTransformResult, CommandExecuteBeforeInput, CommandExecuteBeforeResult,
};
use crate::llm::error::LlmError;
use crate::llm::hook_types::{
    ErrorLlmHookInput, ErrorLlmHookResult, PostLlmHookInput, PostLlmHookResult, PreLlmHookInput,
    PreLlmHookResult,
};
use crate::session::hook_types::{
    SessionClosedHookInput, SessionCreatedHookInput, SessionHookError, SessionHookResult,
    SessionStateHookInput,
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
    #[error("{0}")]
    Chat(#[from] ChatHookError),
    #[error("{0}")]
    Session(#[from] SessionHookError),
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
    SessionState {
        input: SessionStateHookInput,
        metadata: HookInvokeMetadata,
    },
    // Chat hook variants
    ChatSystemTransform {
        input: ChatSystemTransformInput,
        metadata: HookInvokeMetadata,
    },
    ChatMessage {
        input: ChatMessageHookInput,
        metadata: HookInvokeMetadata,
    },
    CommandExecuteBefore {
        input: CommandExecuteBeforeInput,
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
            | Self::SessionClosed { metadata, .. }
            | Self::SessionState { metadata, .. }
            | Self::ChatSystemTransform { metadata, .. }
            | Self::ChatMessage { metadata, .. }
            | Self::CommandExecuteBefore { metadata, .. } => metadata,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HookInvokeOutput {
    /// The primary, category-specific hook result (allow/deny/transform/ack/...).
    pub primary: HookInvokePrimary,
    /// Side-effect actions requested by the plugin alongside `primary`.
    /// Parsed from the plugin's `actions` JSON array; empty for built-in
    /// hookers and for plugins that don't request actions. Dispatchers
    /// execute these after applying `primary` (best-effort, fire-and-forget).
    pub actions: Vec<crate::hook::HookAction>,
}

/// The category-specific result payload of a hook invocation, unwrapped from
/// [`HookInvokeOutput`]. Was previously the top-level `HookInvokeOutput` enum;
/// renamed when `actions` were added as a sibling field.
#[derive(Clone, Debug)]
pub enum HookInvokePrimary {
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
    SessionState(SessionHookResult),
    // Chat hook variants
    ChatSystemTransform(ChatSystemTransformResult),
    ChatMessage(ChatMessageHookResult),
    CommandExecuteBefore(CommandExecuteBeforeResult),
}

impl HookInvokeOutput {
    /// Wrap a primary result with an empty actions list.
    pub fn new(primary: HookInvokePrimary) -> Self {
        Self {
            primary,
            actions: Vec::new(),
        }
    }

    /// Replace the actions list. Convenience for adaptors that parse actions
    /// separately from the primary result.
    pub fn with_actions(mut self, actions: Vec<crate::hook::HookAction>) -> Self {
        self.actions = actions;
        self
    }

    // Category-specific constructors mirroring the previous enum variants.
    // Construction sites that wrote `HookInvokeOutput::Pre(r)` keep working
    // because these are associated functions with the same names. Match
    // sites must destructure via `output.primary` instead.
    #[allow(non_snake_case)]
    pub fn Pre(r: PreHookResult) -> Self {
        Self::new(HookInvokePrimary::Pre(r))
    }
    #[allow(non_snake_case)]
    pub fn Post(r: PostHookResult) -> Self {
        Self::new(HookInvokePrimary::Post(r))
    }
    #[allow(non_snake_case)]
    pub fn Error(r: ErrorHookResult) -> Self {
        Self::new(HookInvokePrimary::Error(r))
    }
    #[allow(non_snake_case)]
    pub fn LlmPre(r: PreLlmHookResult) -> Self {
        Self::new(HookInvokePrimary::LlmPre(r))
    }
    #[allow(non_snake_case)]
    pub fn LlmPost(r: PostLlmHookResult) -> Self {
        Self::new(HookInvokePrimary::LlmPost(r))
    }
    #[allow(non_snake_case)]
    pub fn LlmError(r: ErrorLlmHookResult) -> Self {
        Self::new(HookInvokePrimary::LlmError(r))
    }
    #[allow(non_snake_case)]
    pub fn SessionCreated(r: SessionHookResult) -> Self {
        Self::new(HookInvokePrimary::SessionCreated(r))
    }
    #[allow(non_snake_case)]
    pub fn SessionClosed(r: SessionHookResult) -> Self {
        Self::new(HookInvokePrimary::SessionClosed(r))
    }
    #[allow(non_snake_case)]
    pub fn SessionState(r: SessionHookResult) -> Self {
        Self::new(HookInvokePrimary::SessionState(r))
    }
    #[allow(non_snake_case)]
    pub fn ChatSystemTransform(r: ChatSystemTransformResult) -> Self {
        Self::new(HookInvokePrimary::ChatSystemTransform(r))
    }
    #[allow(non_snake_case)]
    pub fn ChatMessage(r: ChatMessageHookResult) -> Self {
        Self::new(HookInvokePrimary::ChatMessage(r))
    }
    #[allow(non_snake_case)]
    pub fn CommandExecuteBefore(r: CommandExecuteBeforeResult) -> Self {
        Self::new(HookInvokePrimary::CommandExecuteBefore(r))
    }
}
