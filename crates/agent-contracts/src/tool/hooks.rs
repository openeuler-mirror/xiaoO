use crate::hook::registry::Hooker;
use crate::runtime::runtime_view::RuntimeView;
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput,
};
use async_trait::async_trait;

#[async_trait]
pub trait PreToolHook: Hooker {
    async fn hook(&self, input: &PreToolHookInput, runtime: &dyn RuntimeView) -> PreHookResult;
}

#[async_trait]
pub trait PostToolHook: Hooker {
    async fn hook(&self, input: &PostToolHookInput, runtime: &dyn RuntimeView) -> PostHookResult;
}

#[async_trait]
pub trait ErrorToolHook: Hooker {
    async fn hook(&self, input: &ErrorToolHookInput, runtime: &dyn RuntimeView) -> ErrorHookResult;
}
