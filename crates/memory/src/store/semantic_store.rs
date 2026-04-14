use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DurableMemory, DurableMemoryKind, MemoryResult};

use super::DurableMemoryStore;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticSearchQuery {
    pub query_text: String,
    pub limit: usize,
    pub session_id: Option<String>,
    pub kind_filter: Option<DurableMemoryKind>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScoredMemory {
    pub memory: DurableMemory,
    pub score: f64,
}

/// Extends DurableMemoryStore with vector + keyword hybrid search.
///
/// Implementations are expected to store embeddings alongside memories
/// and provide hybrid (vector + FTS) search via `search()`.
#[async_trait]
pub trait SemanticMemoryStore: DurableMemoryStore {
    async fn search(&self, query: &SemanticSearchQuery) -> MemoryResult<Vec<ScoredMemory>>;
    async fn reindex(&self) -> MemoryResult<usize>;
}
