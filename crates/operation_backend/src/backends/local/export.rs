use crate::backends::local::backend::{file_name_string, io_error_for_path, LocalBackendState};
use agent_contracts::backend::{
    capability::{export::ExportFileRequest, OperationExport},
    ExportedFileHandle, ExportedFileMeta, ExportedFileReader, OperationError,
    SharedExportedFileHandle,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct LocalExport {
    _state: Arc<LocalBackendState>,
}

impl LocalExport {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

struct LocalExportedFileHandle {
    host_path: PathBuf,
    metadata: ExportedFileMeta,
}

#[async_trait]
impl ExportedFileHandle for LocalExportedFileHandle {
    fn metadata(&self) -> &ExportedFileMeta {
        &self.metadata
    }

    async fn open_read(&self) -> Result<ExportedFileReader, OperationError> {
        let file = tokio::fs::File::open(self.host_path.as_path())
            .await
            .map_err(|error| io_error_for_path(self.host_path.as_path(), error))?;
        Ok(Box::new(file))
    }
}

#[async_trait]
impl OperationExport for LocalExport {
    async fn export_file(
        &self,
        request: ExportFileRequest,
    ) -> Result<SharedExportedFileHandle, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        self._state.ensure_file(host_path.as_path())?;
        let metadata = std::fs::metadata(host_path.as_path())
            .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
        let file_name = match request.preferred_name {
            Some(name) => name,
            None => file_name_string(host_path.as_path())?,
        };

        Ok(Arc::new(LocalExportedFileHandle {
            host_path,
            metadata: ExportedFileMeta {
                file_name,
                size_bytes: Some(metadata.len()),
                media_type: None,
            },
        }))
    }
}
