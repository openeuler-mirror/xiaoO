//! GlobTool executor implementation.

use async_trait::async_trait;
use std::sync::Arc;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::constants::GLOB_LIMIT;
use super::input::GlobInput;
use super::output::GlobOutput;
use super::spec::GlobToolSpec;
use super::validation;
use super::validation::expand_path;
use crate::r#impl::path_resolver::runtime_workspace_root;

/// Executor for GlobTool.
pub struct GlobToolExecutor {
    spec: Arc<GlobToolSpec>,
}

impl GlobToolExecutor {
    pub fn new(spec: Arc<GlobToolSpec>) -> Self {
        Self { spec }
    }

    /// Executes the glob search operation.
    ///
    /// # Arguments
    /// * `pattern` - The glob pattern to match against
    /// * `search_path` - The directory to search in
    ///
    /// # Returns
    /// * `(files, truncated)` - Tuple of matched files and whether results were truncated
    async fn call_inner(
        &self,
        pattern: &str,
        search_path: &str,
    ) -> Result<(Vec<String>, bool), String> {
        // Build the full pattern with path prefix
        let full_pattern = if search_path.is_empty() || search_path == "." {
            pattern.to_string()
        } else {
            format!("{}/{}", search_path.replace('\\', "/"), pattern)
        };

        let mut files = Vec::new();
        let mut truncated = false;

        // Use glob to find matching files
        for entry in glob::glob(&full_pattern).map_err(|e| e.to_string())? {
            match entry {
                Ok(path) => {
                    if files.len() >= GLOB_LIMIT {
                        truncated = true;
                        break;
                    }
                    // Convert to string representation
                    if let Some(path_str) = path.to_str() {
                        files.push(path_str.to_string());
                    } else {
                        // Handle paths with invalid UTF-8
                        files.push(path.to_string_lossy().into_owned());
                    }
                }
                Err(e) => {
                    // Log error but continue - some patterns may fail on certain files
                    eprintln!("Glob error for entry: {:?}", e);
                }
            }
        }

        Ok((files, truncated))
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

        // Validate input
        let workspace_root = runtime_workspace_root(runtime);
        let validation_result = validation::validate_input_with_base(&input, workspace_root);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);

            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        // Get search path (expand_path, default to cwd)
        let search_path = input
            .path
            .as_ref()
            .map(|p| expand_path(p, workspace_root))
            .unwrap_or_else(|| workspace_root.to_string_lossy().into_owned());

        // Execute glob search
        let start = std::time::Instant::now();
        let result = self.call_inner(&input.pattern, &search_path).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok((files, truncated)) => {
                let num_files = files.len() as u64;
                let output = GlobOutput {
                    duration_ms,
                    num_files,
                    filenames: files,
                    truncated,
                };

                let json_output = serde_json::to_string(&output).map_err(|e| {
                    ToolExecutionError::ExecutionFailed {
                        message: format!("Failed to serialize output: {}", e),
                    }
                })?;

                Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Success {
                        output: json_output,
                    },
                })
            }
            Err(error_message) => Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: error_message,
                },
            }),
        }
    }
}
