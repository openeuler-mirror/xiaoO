use crate::context::budget::TokenBudgetPolicy;
use agent_types::compression::{
    CompressedView, CompressionMeta, ContextAnalysis, MicroCompactResult,
};
use agent_types::ChatMessage;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum CompressionError {
    #[error("compression failed: {0}")]
    Failed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait CompressionPipeline: Send + Sync {
    fn analyze(&self, messages: &[ChatMessage], budget: &dyn TokenBudgetPolicy) -> ContextAnalysis;

    async fn compress(
        &self,
        messages: &[ChatMessage],
        budget: &dyn TokenBudgetPolicy,
        meta: &CompressionMeta,
    ) -> Result<CompressedView, CompressionError>;

    fn microcompact(&self, messages: &[ChatMessage], now_ms: u64) -> MicroCompactResult;
}
