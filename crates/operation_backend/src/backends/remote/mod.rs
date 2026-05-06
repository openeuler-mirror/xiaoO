use agent_contracts::backend::{
    OperationBackend, OperationBackendBuildError, OperationBackendConfig,
};
use std::sync::Arc;

pub async fn build_backend(
    _config: &OperationBackendConfig,
) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
    Err(OperationBackendBuildError::BuildFailed {
        message: "remote backend is not implemented yet".to_string(),
    })
}
