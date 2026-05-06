use crate::backend::capability::{
    OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
};
use crate::backend::OperationError;
use async_trait::async_trait;

/// Identifies the concrete kind of an operation backend, used for capability
/// gating (e.g. LSP requires a local process environment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationBackendKind {
    Local,
    Conch,
    Docker,
    Remote,
}

impl OperationBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationBackendKind::Local => "local",
            OperationBackendKind::Conch => "conch",
            OperationBackendKind::Docker => "docker",
            OperationBackendKind::Remote => "remote",
        }
    }
}

/// Capabilities advertised by an operation backend implementation.
#[derive(Debug, Clone, Copy)]
pub struct OperationBackendCapabilities {
    pub supports_atomic_write: bool,
    pub supports_grep: bool,
    pub supports_export_file: bool,
}

/// Aggregate contract implemented by a concrete execution backend.
/// IMPORTANT: OperationBackend should only be constructed via an OperationBackendBuilder to ensure proper validation and capability gating.
#[async_trait]
pub trait OperationBackend: Send + Sync {
    /// Stable identifier for logging and diagnostics.
    fn backend_id(&self) -> &str;

    /// The kind of this backend implementation.
    fn backend_kind(&self) -> OperationBackendKind;

    /// Advertised capability metadata for gating and fail-fast decisions.
    fn capabilities(&self) -> OperationBackendCapabilities;

    fn paths(&self) -> &dyn OperationPathResolver;
    fn files(&self) -> &dyn OperationFileSystem;
    fn search(&self) -> &dyn OperationSearch;
    fn exec(&self) -> &dyn OperationExec;
    fn export(&self) -> &dyn OperationExport;
    async fn shutdown(&self) -> Result<(), OperationError>;
}
