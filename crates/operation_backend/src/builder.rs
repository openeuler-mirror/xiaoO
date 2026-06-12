use agent_contracts::backend::{
    OperationBackend, OperationBackendBuildError, OperationBackendBuildInput,
    OperationBackendBuilder, OperationBackendConfig,
};
use async_trait::async_trait;
use serde_json::json;
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
        input: &OperationBackendBuildInput,
    ) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
        let config = resolve_backend_config(input)?;
        match config.kind.as_str() {
            "local" => crate::backends::local::build_backend(&config).await,
            other => Err(OperationBackendBuildError::UnsupportedBackend {
                kind: other.to_string(),
            }),
        }
    }
}

fn resolve_backend_config(
    input: &OperationBackendBuildInput,
) -> Result<OperationBackendConfig, OperationBackendBuildError> {
    if let Some(config) = &input.config {
        return Ok(config.clone());
    }

    let workspace_root =
        input
            .workspace_root
            .as_ref()
            .ok_or_else(|| OperationBackendBuildError::InvalidConfig {
                message: "workspace_root is required when operation_backend config is absent"
                    .to_string(),
            })?;

    Ok(OperationBackendConfig::new(
        "local",
        json!({
            "workspace_root": workspace_root.display().to_string(),
            "home_dir": std::env::var_os("HOME").map(|path| path.to_string_lossy().to_string()),
            "temp_root": std::env::temp_dir().display().to_string(),
        }),
    ))
}
