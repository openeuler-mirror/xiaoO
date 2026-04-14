use async_trait::async_trait;

use crate::MemoryResult;

/// Embedding vector generation abstraction.
///
/// Lives in the memory crate because only memory consumes embeddings.
/// Follows the same pattern as LlmProvider in llm-client.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, texts: &[&str]) -> MemoryResult<Vec<Vec<f32>>>;

    async fn embed_one(&self, text: &str) -> MemoryResult<Vec<f32>> {
        let mut results = self.embed(&[text]).await?;
        results.pop().ok_or_else(|| crate::MemoryError::Embedding {
            message: "embedding provider returned empty result".to_string(),
        })
    }
}
