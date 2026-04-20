use crate::backend::{BackendPath, OperationError};
use async_trait::async_trait;

/// Request to resolve a raw path string.
#[derive(Debug, Clone)]
pub struct ResolvePathRequest {
    pub raw_path: String,
    pub base: ResolveBase,
}

/// The base directory for path resolution.
#[derive(Debug, Clone)]
pub enum ResolveBase {
    WorkspaceRoot,
    HomeDir,
    Explicit(BackendPath),
}

/// Path resolution capability.
#[async_trait]
pub trait OperationPathResolver: Send + Sync {
    /// The workspace root directory.
    fn workspace_root(&self) -> &BackendPath;
    /// The home directory, if available.
    fn home_dir(&self) -> Option<&BackendPath>;

    /// Resolve a raw path string to a backend path.
    async fn resolve_path(
        &self,
        request: ResolvePathRequest,
    ) -> Result<BackendPath, OperationError>;
}
