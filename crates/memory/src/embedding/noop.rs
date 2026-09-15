use async_trait::async_trait;

use crate::MemoryResult;

use super::EmbeddingProvider;

/// Zero-dimension embedding provider — keyword-only fallback.
/// Returns empty vectors, disabling vector search while keeping FTS5 functional.
pub struct NoopEmbedding;

#[async_trait]
impl EmbeddingProvider for NoopEmbedding {
    fn name(&self) -> &str {
        "noop"
    }

    fn dimensions(&self) -> usize {
        0
    }

    async fn embed(&self, texts: &[&str]) -> MemoryResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| Vec::new()).collect())
    }

    async fn embed_one(&self, _text: &str) -> MemoryResult<Vec<f32>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/memory/embedding/noop_test.rs"]
mod tests;
