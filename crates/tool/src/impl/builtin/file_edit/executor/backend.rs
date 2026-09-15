use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_contracts::backend::capability::filesystem::{ReadBytesRequest, WriteBytesRequest};
use agent_contracts::backend::capability::path::ResolveBase;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::super::validation::backend as validation;
use super::input::FileEditInput;
use super::output::FileEditOutput;
use super::spec::FileEditToolSpec;
use super::utils::{
    apply_edit_to_file, find_actual_string, get_patch_for_edit, preserve_quote_style,
};
use crate::r#impl::builtin::file_read::dedup::{system_time_to_timestamp, DedupStateStore};
use crate::r#impl::builtin::preferred_overwrite_mode;
use crate::r#impl::fs_timeout::{timed, DEFAULT_FS_TIMEOUT_MS};
use crate::r#impl::lsp_hooks::{fetch_diagnostics, spawn_touch_file};
use crate::r#impl::ToolRuntimeServices;

const LSP_DIAG_TIMEOUT_SECS: u64 = 15;

pub struct FileEditExecutor {
    spec: Arc<FileEditToolSpec>,
    dedup_store: Arc<Mutex<DedupStateStore>>,
    services: ToolRuntimeServices,
}

impl FileEditExecutor {
    pub fn new(spec: Arc<FileEditToolSpec>, services: ToolRuntimeServices) -> Self {
        Self::new_with_state(spec, services, Arc::new(Mutex::new(DedupStateStore::new())))
    }

    pub(crate) fn new_with_state(
        spec: Arc<FileEditToolSpec>,
        services: ToolRuntimeServices,
        dedup_store: Arc<Mutex<DedupStateStore>>,
    ) -> Self {
        Self {
            spec,
            dedup_store,
            services,
        }
    }

    async fn get_dedup_store(&self) -> tokio::sync::MutexGuard<'_, DedupStateStore> {
        self.dedup_store.lock().await
    }
}

impl Default for FileEditExecutor {
    fn default() -> Self {
        Self::new(
            Arc::new(FileEditToolSpec::new()),
            ToolRuntimeServices::default(),
        )
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

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "file_edit requires operation backend access, but none is configured"
                        .to_string(),
                },
            });
        }
        let backend = backend.unwrap();
        let lsp = self
            .services
            .lsp_registry
            .as_ref()
            .and_then(|reg| reg.get_or_create(Arc::clone(&backend)));

        let resolved = timed(
            "file_edit resolve_path",
            DEFAULT_FS_TIMEOUT_MS,
            backend.paths().resolve_path(
                agent_contracts::backend::capability::path::ResolvePathRequest {
                    raw_path: input.file_path.trim().to_string(),
                    base: ResolveBase::WorkspaceRoot,
                },
            ),
        )
        .await
        .map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to resolve path: {}", e),
        })?;

        let resolved_str = resolved.to_string();

        let stat = timed(
            "file_edit stat",
            DEFAULT_FS_TIMEOUT_MS,
            backend.files().stat(&resolved),
        )
        .await
        .map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to stat file: {}", e),
        })?;

        let file_content = if stat.exists {
            let bytes = timed(
                "file_edit read_bytes",
                DEFAULT_FS_TIMEOUT_MS,
                backend.files().read_bytes(ReadBytesRequest {
                    path: resolved.clone(),
                }),
            )
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to read file: {}", e),
            })?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };

        let mtime = system_time_to_timestamp(stat.modified_at);

        let validation_result = {
            let dedup_store = self.get_dedup_store().await;
            validation::validate_input_backend(
                &input,
                file_content.as_deref(),
                &dedup_store,
                &resolved_str,
                &stat,
                mtime,
            )
        };
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
            let capabilities = backend.capabilities();
            let write_mode = preferred_overwrite_mode(capabilities.supports_atomic_write);

            timed(
                "file_edit write_bytes",
                DEFAULT_FS_TIMEOUT_MS,
                backend.files().write_bytes(WriteBytesRequest {
                    path: resolved,
                    content: input.new_string.as_bytes().to_vec(),
                    mode: write_mode,
                }),
            )
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to write file: {}", e),
            })?;

            if let Some(ref lsp) = lsp {
                spawn_touch_file(lsp, std::path::Path::new(&resolved_str));
            }

            let lsp_diagnostics = if let Some(ref lsp) = lsp {
                fetch_diagnostics(
                    lsp,
                    std::path::Path::new(&resolved_str),
                    LSP_DIAG_TIMEOUT_SECS,
                )
                .await
            } else {
                None
            };

            let output = FileEditOutput {
                file_path: input.file_path.clone(),
                new_lines: input.new_string.lines().count() as u32,
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
            message: format!("File not found: {}", resolved_str),
        })?;

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

        let capabilities = backend.capabilities();
        let write_mode = preferred_overwrite_mode(capabilities.supports_atomic_write);

        timed(
            "file_edit write_bytes",
            DEFAULT_FS_TIMEOUT_MS,
            backend.files().write_bytes(WriteBytesRequest {
                path: resolved,
                content: updated_content.as_bytes().to_vec(),
                mode: write_mode,
            }),
        )
        .await
        .map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to write file: {}", e),
        })?;

        if let Some(ref lsp) = lsp {
            spawn_touch_file(lsp, std::path::Path::new(&resolved_str));
        }

        let lsp_diagnostics = if let Some(ref lsp) = lsp {
            fetch_diagnostics(
                lsp,
                std::path::Path::new(&resolved_str),
                LSP_DIAG_TIMEOUT_SECS,
            )
            .await
        } else {
            None
        };

        let output = FileEditOutput {
            file_path: input.file_path.clone(),
            new_lines: updated_content.lines().count() as u32,
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

#[cfg(test)]
#[path = "../../../../../../../tests/unit/tool/impl/builtin/file_edit/executor/backend_test.rs"]
mod tests;
