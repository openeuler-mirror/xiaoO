use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use similar::{ChangeTag, TextDiff};

use agent_contracts::backend::capability::filesystem::{
    ReadBytesRequest, WriteBytesRequest, WriteMode,
};
use agent_contracts::backend::capability::path::ResolveBase;
use agent_contracts::backend::BackendPath;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::super::validation::backend as validation;
use super::input::FileWriteInput;
use super::output::{CreateOutput, FileWriteOutput, Hunk, StructuredPatch, UpdateOutput};
use super::spec::FileWriteToolSpec;
use crate::r#impl::lsp_hooks::{fetch_diagnostics, spawn_touch_file};
use crate::r#impl::ToolRuntimeServices;

const LSP_DIAG_TIMEOUT_SECS: u64 = 15;

pub struct FileWriteExecutor {
    spec: Arc<FileWriteToolSpec>,
    services: ToolRuntimeServices,
}

impl FileWriteExecutor {
    pub fn new(spec: Arc<FileWriteToolSpec>, services: ToolRuntimeServices) -> Self {
        Self { spec, services }
    }

    fn normalize_path(file_path: &str) -> String {
        file_path.trim().to_string()
    }

    fn generate_structured_patch(old_content: &str, new_content: &str) -> StructuredPatch {
        let diff = TextDiff::from_lines(old_content, new_content);

        let mut hunks = Vec::new();
        let mut updated_lines = Vec::new();
        let mut has_changes = false;

        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Delete | ChangeTag::Insert => {
                    has_changes = true;
                    let sign = if change.tag() == ChangeTag::Delete {
                        "-"
                    } else {
                        "+"
                    };
                    updated_lines.push(format!("{}{}", sign, change));
                }
                ChangeTag::Equal => {
                    updated_lines.push(format!(" {}", change));
                }
            }
        }

        if !has_changes {
            return StructuredPatch { hunks: None };
        }

        let old_lines_count = old_content.lines().count() as u32;
        let new_lines_count = new_content.lines().count() as u32;

        hunks.push(Hunk {
            old_start: 1,
            old_lines: old_lines_count,
            new_start: 1,
            new_lines: new_lines_count,
            lines: updated_lines,
        });

        StructuredPatch { hunks: Some(hunks) }
    }

    fn parent_backend_path(path: &BackendPath) -> Option<BackendPath> {
        Path::new(path.0.as_str())
            .parent()
            .and_then(|parent| parent.to_str())
            .map(|parent| BackendPath(parent.to_string()))
    }
}

#[async_trait]
impl ToolExecutor for FileWriteExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: FileWriteInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "file_write requires operation backend access, but none is configured"
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

        let file_path = Self::normalize_path(&input.file_path);

        let validation_result = validation::validate_input_with_base_from_bytes(&input);
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

        let resolved = backend
            .paths()
            .resolve_path(
                agent_contracts::backend::capability::path::ResolvePathRequest {
                    raw_path: file_path.clone(),
                    base: ResolveBase::WorkspaceRoot,
                },
            )
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to resolve path: {}", e),
            })?;

        let stat = backend.files().stat(&resolved).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to stat file: {}", e),
            }
        })?;

        let original_content = if stat.exists {
            let bytes = backend
                .files()
                .read_bytes(ReadBytesRequest {
                    path: resolved.clone(),
                })
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed {
                    message: format!("Failed to read existing file: {}", e),
                })?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };

        let structured_patch = match &original_content {
            Some(old) => Self::generate_structured_patch(old, &input.content),
            None => StructuredPatch { hunks: None },
        };

        let capabilities = backend.capabilities();
        let write_mode = if capabilities.supports_atomic_write {
            WriteMode::AtomicOverwrite
        } else {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message:
                        "file_write requires atomic write support, but backend does not support it"
                            .to_string(),
                },
            });
        };

        if original_content.is_none() {
            if let Some(parent) = Self::parent_backend_path(&resolved) {
                backend.files().create_dir_all(&parent).await.map_err(|e| {
                    ToolExecutionError::ExecutionFailed {
                        message: format!("Failed to create parent directory: {}", e),
                    }
                })?;
            }
        }

        let _write_result = backend
            .files()
            .write_bytes(WriteBytesRequest {
                path: resolved.clone(),
                content: input.content.as_bytes().to_vec(),
                mode: write_mode,
            })
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to write file: {}", e),
            })?;

        let resolved_path = Path::new(resolved.0.as_str());
        if let Some(ref lsp) = lsp {
            spawn_touch_file(lsp, resolved_path);
        }

        let lsp_diagnostics = if let Some(ref lsp) = lsp {
            fetch_diagnostics(lsp, resolved_path, LSP_DIAG_TIMEOUT_SECS).await
        } else {
            None
        };

        let output: FileWriteOutput = match original_content {
            Some(old_content) => FileWriteOutput::Update(UpdateOutput {
                file_path: input.file_path.clone(),
                content: input.content.clone(),
                structured_patch,
                original_file: old_content,
                git_diff: None,
                lsp_diagnostics: lsp_diagnostics.clone(),
            }),
            None => FileWriteOutput::Create(CreateOutput {
                file_path: input.file_path.clone(),
                content: input.content.clone(),
                structured_patch,
                original_file: serde_json::Value::Null,
                git_diff: None,
                lsp_diagnostics,
            }),
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
