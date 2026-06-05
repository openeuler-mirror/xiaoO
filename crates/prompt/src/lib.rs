pub mod builder_impl;
pub mod compose;
pub mod context;
pub mod decision;

pub use agent_contracts::{PromptBuildInput, PromptBuilder, ToolSpecView};
pub use agent_types::context::prompt::{
    EnvironmentInfo, MemorySnippet, PromptBuildError, PromptBuildResult, SkillSummary,
};
pub use builder_impl::PromptBuilderImpl;
pub use compose::{compose_channel_system_prompt, compose_system_text, ChannelPromptSections};
pub use context::{collect_prompt_context, CompressedHistory, InstructionContext, PromptContext};
pub use decision::{decide_prompt, PromptAction, PromptDecision, PromptState, ToolMode};
