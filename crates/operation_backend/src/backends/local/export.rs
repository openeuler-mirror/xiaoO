use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{export::ExportFileRequest, OperationExport},
    ExportedFile, OperationError,
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
        _request: ExportFileRequest,
    ) -> Result<ExportedFile, OperationError> {
        todo!("local export is not implemented yet")
    }
}
