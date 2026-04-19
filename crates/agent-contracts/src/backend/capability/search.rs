use crate::backend::{BackendPath, OperationError};
use async_trait::async_trait;

/// Request for glob pattern matching.
#[derive(Debug, Clone)]
pub struct GlobRequest {
    pub pattern: String,
    pub base_dir: Option<BackendPath>,
    pub limit: Option<usize>,
}

/// Request for content search.
#[derive(Debug, Clone)]
pub struct GrepRequest {
    pub query: String,
    pub base_dir: BackendPath,
    pub include: Option<String>,
    pub mode: GrepMode,
    pub head_limit: Option<usize>,
}

/// Mode for grep operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepMode {
    FilesWithMatches,
    Content,
    Count,
}

/// Result of a grep operation.
#[derive(Debug, Clone)]
pub struct GrepResult {
    pub entries: Vec<String>,
}

/// Search operations capability.
#[async_trait]
pub trait OperationSearch: Send + Sync {
    /// Glob pattern matching.
    async fn glob(&self, request: GlobRequest) -> Result<Vec<BackendPath>, OperationError>;

    /// Content search.
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, OperationError>;
}
