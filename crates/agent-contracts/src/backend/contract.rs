use crate::backend::capability::{
    OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
};
use crate::backend::OperationError;
use async_trait::async_trait;

/// Capabilities advertised by an operation backend implementation.
#[derive(Debug, Clone, Copy)]
pub struct OperationBackendCapabilities {
    pub supports_atomic_write: bool,
    pub supports_grep: bool,
    pub supports_export_file: bool,
    pub supports_lsp: bool,
}

/// Aggregate contract implemented by a concrete execution backend.
#[async_trait]
pub trait OperationBackend: Send + Sync {
    /// Stable identifier for logging and diagnostics.
    fn backend_id(&self) -> &str;

    /// Advertised capability metadata for gating and fail-fast decisions.
    fn capabilities(&self) -> OperationBackendCapabilities;

    fn paths(&self) -> &dyn OperationPathResolver;
    fn files(&self) -> &dyn OperationFileSystem;
    fn search(&self) -> &dyn OperationSearch;
    fn exec(&self) -> &dyn OperationExec;
    fn export(&self) -> &dyn OperationExport;
    async fn shutdown(&self) -> Result<(), OperationError>;
}
