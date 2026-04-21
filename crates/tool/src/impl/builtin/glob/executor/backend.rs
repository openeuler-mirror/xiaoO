use async_trait::async_trait;
use std::sync::Arc;

use agent_contracts::backend::capability::search::GlobRequest;
use agent_contracts::backend::{BackendPath, PathKind};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::constants::GLOB_LIMIT;
use super::input::GlobInput;
use super::output::GlobOutput;
use super::spec::GlobToolSpec;

pub struct GlobToolExecutor {
    spec: Arc<GlobToolSpec>,
}

impl GlobToolExecutor {
    pub fn new(spec: Arc<GlobToolSpec>) -> Self {
        Self { spec }
    }

    fn is_unc_path(path: &str) -> bool {
        path.starts_with("\\\\") || path.starts_with("//")
    }

    async fn resolve_and_validate_base_dir(
        path: Option<&str>,
        backend: &dyn agent_contracts::backend::OperationBackend,
    ) -> Result<Option<BackendPath>, String> {
        let Some(path) = path else {
            return Ok(None);
        };

        let path_str = path.trim();

        if Self::is_unc_path(path_str) {
            return Err("UNC paths are not allowed for backend glob execution".to_string());
        }

        let resolved = backend
            .paths()
            .resolve_path(
                agent_contracts::backend::capability::path::ResolvePathRequest {
                    raw_path: path_str.to_string(),
                    base: agent_contracts::backend::capability::path::ResolveBase::WorkspaceRoot,
                },
            )
            .await
            .map_err(|e| format!("Failed to resolve path: {}", e))?;

        let stat = backend
            .files()
            .stat(&resolved)
            .await
            .map_err(|e| format!("Failed to stat path: {}", e))?;

        if !stat.exists {
            return Err(format!("Directory does not exist: {}", path));
        }

        if stat.kind != Some(PathKind::Directory) {
            return Err(format!("Path is not a directory: {}", path));
        }

        Ok(Some(resolved))
    }

    fn format_output(
        paths: &[BackendPath],
        truncated: bool,
        duration_ms: u64,
    ) -> Result<String, String> {
        let filenames: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let num_files = filenames.len() as u64;

        let output = GlobOutput {
            duration_ms,
            num_files,
            filenames,
            truncated,
        };

        serde_json::to_string(&output).map_err(|e| format!("Failed to serialize output: {}", e))
    }
}

#[async_trait]
impl ToolExecutor for GlobToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: GlobInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "glob requires operation backend access, but none is configured"
                        .to_string(),
                },
            });
        }
        let backend = backend.unwrap();

        let base_dir = Self::resolve_and_validate_base_dir(input.path.as_deref(), backend)
            .await
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        let start = std::time::Instant::now();

        let request = GlobRequest {
            pattern: input.pattern.clone(),
            base_dir,
            limit: Some(GLOB_LIMIT),
        };

        let result = backend.search().glob(request).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Backend glob failed: {}", e),
            }
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let truncated = result.len() >= GLOB_LIMIT;

        let output = Self::format_output(&result, truncated, duration_ms)
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output },
        })
    }
}
