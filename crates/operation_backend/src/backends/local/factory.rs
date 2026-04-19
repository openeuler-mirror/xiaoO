use crate::backends::local::backend::{LocalBackendState, LocalOperationBackend};
use agent_contracts::backend::{
    OperationBackend, OperationBackendBuildError, OperationBackendConfig,
};
use std::sync::Arc;

pub async fn build_backend(
    _config: &OperationBackendConfig,
) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
    let _backend = LocalOperationBackend::new(Arc::new(LocalBackendState {
        backend_id: "local".to_string(),
    }));

    Err(OperationBackendBuildError::BuildFailed {
        message: "local backend skeleton is not implemented yet".to_string(),
    })
}
