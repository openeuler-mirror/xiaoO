use crate::backends;
use agent_contracts::backend::{
    OperationBackend, OperationBackendBuildError, OperationBackendBuilder, OperationBackendConfig,
};
use async_trait::async_trait;
use std::sync::Arc;

pub struct OperationBackendBuilderImpl;

impl OperationBackendBuilderImpl {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OperationBackendBuilderImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OperationBackendBuilder for OperationBackendBuilderImpl {
    async fn build(
        &self,
        config: &OperationBackendConfig,
    ) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
        match config.kind.as_str() {
            "local" => backends::local::build_backend(config).await,
            "docker" => backends::docker::build_backend(config).await,
            "remote" => backends::remote::build_backend(config).await,
            other => Err(OperationBackendBuildError::UnsupportedBackend {
                kind: other.to_string(),
            }),
        }
    }
}
