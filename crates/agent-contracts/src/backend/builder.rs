use crate::backend::config::OperationBackendConfig;
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
        config: &OperationBackendConfig,
    ) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError>;
}
