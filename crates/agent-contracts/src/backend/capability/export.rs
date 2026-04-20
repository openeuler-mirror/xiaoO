use crate::backend::{BackendPath, OperationError, SharedExportedFileHandle};
use async_trait::async_trait;

/// Request to export a file.
#[derive(Debug, Clone)]
pub struct ExportFileRequest {
    pub path: BackendPath,
    pub preferred_name: Option<String>,
}

/// File export capability.
#[async_trait]
pub trait OperationExport: Send + Sync {
    /// Export a file for external use (e.g., sending via channel).
    async fn export_file(&self, request: ExportFileRequest)
        -> Result<SharedExportedFileHandle, OperationError>;
}
