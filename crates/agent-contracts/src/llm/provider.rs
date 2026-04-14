use agent_types::{LlmError, LlmRequest, LlmResponse, StreamChunk};
use async_trait::async_trait;

#[derive(Clone, Debug)]
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tool_calls: bool,
    pub supports_json_mode: bool,
    pub max_context_window: usize,
    pub model_name: String,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError>;

    fn capabilities(&self) -> &ProviderCapabilities;
}
