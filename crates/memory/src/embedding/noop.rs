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
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_returns_empty_vectors() {
        let noop = NoopEmbedding;
        assert_eq!(noop.name(), "noop");
        assert_eq!(noop.dimensions(), 0);

        let result = noop.embed(&["hello", "world"]).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result[0].is_empty());

        let single = noop.embed_one("test").await.unwrap();
        assert!(single.is_empty());
    }
}
