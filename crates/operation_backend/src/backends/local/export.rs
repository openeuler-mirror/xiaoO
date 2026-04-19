use crate::backends::local::backend::{file_name_string, io_error_for_path, LocalBackendState};
use agent_contracts::backend::{
    capability::{export::ExportFileRequest, OperationExport},
    ExportedFile, ExportedFileSource, OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;

pub(crate) struct LocalExport {
    _state: Arc<LocalBackendState>,
}

impl LocalExport {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationExport for LocalExport {
    async fn export_file(
        &self,
        request: ExportFileRequest,
    ) -> Result<ExportedFile, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        self._state.ensure_file(host_path.as_path())?;
        let metadata = std::fs::metadata(host_path.as_path())
            .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
        let file_name = match request.preferred_name {
            Some(name) => name,
            None => file_name_string(host_path.as_path())?,
        };

        Ok(ExportedFile {
            file_name,
            size_bytes: metadata.len(),
            media_type: None,
            source: ExportedFileSource::HostPath(host_path),
        })
    }
}
