use crate::backend::config::OperationBackendBuildInput;
use crate::backend::contract::OperationBackend;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum OperationBackendBuildError {
    #[error("invalid backend config: {message}")]
    InvalidConfig { message: String },

    #[error("unsupported backend kind: {kind}")]
    UnsupportedBackend { kind: String },

    #[error("backend build failed: {message}")]
    BuildFailed { message: String },
}

#[async_trait]
pub trait OperationBackendBuilder: Send + Sync {
    async fn build(
        &self,
        input: &OperationBackendBuildInput,
    ) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError>;
}
