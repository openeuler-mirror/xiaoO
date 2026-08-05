use crate::backend::capability::{
    OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
};
use crate::backend::{OperationError, OperationPermissionControl};
use crate::InteractionHandle;
use async_trait::async_trait;
use std::sync::Arc;

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
    /// Hand the backend an interaction handle it may use to ask the user
    /// questions at runtime (e.g. sandbox permission prompts). Backends that
    /// never surface user prompts leave the default no-op; wrapper backends
    /// forward to their inner.
    fn attach_interaction(&self, _interaction: Arc<dyn InteractionHandle>) {}
    fn permission_control(&self) -> Option<&dyn OperationPermissionControl> {
        None
    }
    async fn shutdown(&self) -> Result<(), OperationError>;
}
