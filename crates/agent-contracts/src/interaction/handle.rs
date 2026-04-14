use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

#[async_trait]
pub trait InteractionHandle: Send + Sync {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse;
}
