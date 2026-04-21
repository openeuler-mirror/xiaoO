//! FileEditTool executor implementation.
#![allow(unused_imports)]

use similar::{ChangeTag, TextDiff};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::input::FileEditInput;
use super::output::FileEditOutput;
use super::spec::FileEditToolSpec;
use super::utils::{
    apply_edit_to_file, find_actual_string, get_patch_for_edit, preserve_quote_style,
};
use super::validation;
use super::validation::expand_path;
use crate::r#impl::builtin::file_read::dedup::DedupStateStore;
use crate::r#impl::lsp_hooks::{fetch_diagnostics, spawn_touch_file};
use crate::r#impl::path_resolver::runtime_workspace_root;
use crate::r#impl::ToolRuntimeServices;

const LSP_DIAG_TIMEOUT_SECS: u64 = 15;

/// Executor for FileEditTool.
pub struct FileEditExecutor {
    spec: Arc<FileEditToolSpec>,
    dedup_store: Mutex<DedupStateStore>,
    services: ToolRuntimeServices,
}

impl FileEditExecutor {
    /// Creates a new FileEditExecutor.
    pub fn new(spec: Arc<FileEditToolSpec>, services: ToolRuntimeServices) -> Self {
        Self {
            spec,
            dedup_store: Mutex::new(DedupStateStore::new()),
            services,
        }
    }

    async fn get_dedup_store(&self) -> tokio::sync::MutexGuard<'_, DedupStateStore> {
        self.dedup_store.lock().await
    }

    fn read_file_content(file_path: &str) -> Result<String, ToolExecutionError> {
        std::fs::read_to_string(file_path).map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to read file {}: {}", file_path, e),
        })
    }

    fn write_file_content(file_path: &str, content: &str) -> Result<(), ToolExecutionError> {
        std::fs::write(file_path, content).map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to write file {}: {}", file_path, e),
        })
    }
}

impl Default for FileEditExecutor {
    fn default() -> Self {
        Self::new(Arc::new(FileEditToolSpec::new()), ToolRuntimeServices::default())
    }
}

#[async_trait]
impl ToolExecutor for FileEditExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: FileEditInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let dedup_store = self.get_dedup_store().await;
        let workspace_root = runtime_workspace_root(runtime);

        let expanded_path = expand_path(&input.file_path, workspace_root);
        let file_content = if std::path::Path::new(&expanded_path).exists() {
            Some(Self::read_file_content(&expanded_path)?)
        } else {
            None
        };

        let validation_result = validation::validate_input(
            &input,
            file_content.as_deref(),
            Some(&dedup_store),
            workspace_root,
        );
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
        if input.old_string.is_empty() {
            let new_content = &input.new_string;

            Self::write_file_content(&expanded_path, new_content)?;

            // Notify LSP immediately (fire-and-forget) so it starts indexing the new content.
            if let Some(lsp) = &self.services.lsp_service {
                spawn_touch_file(lsp, std::path::Path::new(&expanded_path));
            }

            let lsp_diagnostics = if let Some(lsp) = &self.services.lsp_service {
                fetch_diagnostics(lsp, std::path::Path::new(&expanded_path), LSP_DIAG_TIMEOUT_SECS).await
            } else {
                None
            };

            let output = FileEditOutput {
                file_path: input.file_path.clone(),
                old_string: String::new(),
                new_string: input.new_string.clone(),
                original_file: String::new(),
                structured_patch: Vec::new(),
                user_modified: false,
                replace_all: false,
                git_diff: None,
                lsp_diagnostics,
            };

            let json_output = serde_json::to_string(&output).map_err(|e| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("Failed to serialize output: {}", e),
                }
            })?;
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success {
                    output: json_output,
                },
            });
        }

        let content = file_content.ok_or_else(|| ToolExecutionError::ExecutionFailed {
            message: format!("File not found: {}", expanded_path),
        })?;

        let original_file = content.clone();

        let actual_old_string =
            find_actual_string(&content, &input.old_string).ok_or_else(|| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("old_string not found in file: {}", input.old_string),
                }
            })?;

        let styled_new_string = preserve_quote_style(&actual_old_string, &input.new_string);

        let updated_content = apply_edit_to_file(
            &content,
            &actual_old_string,
            &styled_new_string,
            input.replace_all,
        )
        .ok_or_else(|| ToolExecutionError::ExecutionFailed {
            message: "Failed to apply edit: old_string not found in file".to_string(),
        })?;

        let (structured_patch, _updated_file) =
            get_patch_for_edit(&actual_old_string, &styled_new_string);

        Self::write_file_content(&expanded_path, &updated_content)?;

        // Notify LSP immediately (fire-and-forget) so it starts indexing the new content.
        if let Some(lsp) = &self.services.lsp_service {
            spawn_touch_file(lsp, std::path::Path::new(&expanded_path));
        }

        let lsp_diagnostics = if let Some(lsp) = &self.services.lsp_service {
            fetch_diagnostics(lsp, std::path::Path::new(&expanded_path), LSP_DIAG_TIMEOUT_SECS).await
        } else {
            None
        };

        let output = FileEditOutput {
            file_path: input.file_path.clone(),
            old_string: actual_old_string,
            new_string: styled_new_string,
            original_file,
            structured_patch,
            user_modified: false,
            replace_all: input.replace_all,
            git_diff: None,
            lsp_diagnostics,
        };

        let json_output =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success {
                output: json_output,
            },
        })
    }
}
