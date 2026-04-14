use super::input::PromptBuildInput;
use agent_types::context::prompt::{PromptBuildError, PromptBuildResult};
use async_trait::async_trait;

#[async_trait]
pub trait PromptBuilder: Send + Sync {
    async fn build(&self, input: PromptBuildInput) -> Result<PromptBuildResult, PromptBuildError>;
}
