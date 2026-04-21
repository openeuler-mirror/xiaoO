use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_contracts::backend::capability::filesystem::{
    ReadBytesRequest, TempPathKind, TempPathRequest, WriteBytesRequest, WriteMode,
};
use agent_contracts::backend::capability::path::ResolveBase;
use agent_contracts::backend::BackendPath;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::super::validation::backend as validation;
use super::constants::{
    DEFAULT_MAX_SIZE_BYTES, DEFAULT_MAX_TOKENS, IMAGE_EXTENSIONS, NOTEBOOK_EXTENSION, PDF_EXTENSION,
};
use super::dedup::{system_time_to_timestamp, DedupStateStore, FileReadState};
use super::input::FileReadInput;
use super::output::FileReadOutput;
use super::readers;
use super::spec::FileReadToolSpec;
use crate::r#impl::lsp_hooks::spawn_touch_file;
use crate::r#impl::ToolRuntimeServices;

pub struct FileReadExecutor {
    spec: Arc<FileReadToolSpec>,
    dedup_store: Mutex<DedupStateStore>,
    services: ToolRuntimeServices,
}

impl FileReadExecutor {
    pub fn new(spec: Arc<FileReadToolSpec>, services: ToolRuntimeServices) -> Self {
        Self {
            spec,
            dedup_store: Mutex::new(DedupStateStore::new()),
            services,
        }
    }

    fn normalize_path(file_path: &str) -> String {
        file_path.trim().to_string()
    }

    fn get_extension(file_path: &str) -> Option<String> {
        Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
    }

    fn is_pdf_extension(ext: &str) -> bool {
        ext.eq_ignore_ascii_case(PDF_EXTENSION)
    }

    fn is_image_extension(ext: &str) -> bool {
        IMAGE_EXTENSIONS
            .iter()
            .any(|&e| e.eq_ignore_ascii_case(ext))
    }

    fn is_notebook_extension(ext: &str) -> bool {
        ext.eq_ignore_ascii_case(NOTEBOOK_EXTENSION)
    }

    fn dedup_applies(ext: &str) -> bool {
        !Self::is_image_extension(ext) && !Self::is_pdf_extension(ext)
    }

    async fn get_dedup_store(&self) -> tokio::sync::MutexGuard<'_, DedupStateStore> {
        self.dedup_store.lock().await
    }

    async fn process_notebook(
        &self,
        file_path: &str,
        _ext: &str,
        bytes: &[u8],
        input: &FileReadInput,
        max_size_bytes: u64,
        max_tokens: usize,
        mtime: i64,
    ) -> Result<FileReadOutput, String> {
        let cells =
            readers::notebook::read_notebook_from_bytes(file_path, bytes, Some(max_size_bytes))
                .map_err(|e| e.to_string())?;

        let cells_json = serde_json::to_string(&cells.cells).map_err(|e| e.to_string())?;
        let cells_json_bytes = cells_json.len() as u64;

        if cells_json_bytes > max_size_bytes {
            return Err(format!(
                "Notebook content ({} bytes) exceeds maximum allowed size ({} bytes).",
                cells_json_bytes, max_size_bytes
            ));
        }

        let estimated_tokens = super::tokenizer::estimate_tokens(file_path, &cells_json);
        if estimated_tokens > max_tokens {
            return Err(format!(
                "Notebook content token count ({}) exceeds maximum ({})",
                estimated_tokens, max_tokens
            ));
        }

        let mut dedup_store = self.get_dedup_store().await;
        dedup_store.set_read_state(
            file_path.to_string(),
            FileReadState {
                timestamp: mtime,
                offset: input.offset,
                limit: input.limit,
                is_partial_view: input.offset.is_some() || input.limit.is_some(),
            },
        );

        Ok(FileReadOutput::Notebook(cells))
    }

    fn process_image(
        _file_path: &str,
        ext: &str,
        bytes: &[u8],
        max_tokens: usize,
    ) -> Result<FileReadOutput, String> {
        let image_output = readers::image::read_image_from_bytes(ext, bytes, Some(max_tokens))
            .map_err(|e| e.to_string())?;

        Ok(FileReadOutput::Image(image_output))
    }

    async fn process_pdf(
        &self,
        file_path: &str,
        _ext: &str,
        bytes: &[u8],
        input: &FileReadInput,
        backend: &dyn agent_contracts::backend::OperationBackend,
    ) -> Result<FileReadOutput, String> {
        if let Some(ref pages) = input.pages {
            let temp_path = backend
                .files()
                .temp_path(TempPathRequest {
                    kind: TempPathKind::Directory,
                    preferred_parent: None,
                    prefix: Some("pdf_pages".to_string()),
                    suffix: None,
                })
                .await
                .map_err(|e| format!("Failed to create temp directory: {}", e))?;

            let output_dir_str = temp_path.to_string();
            let (parts, files) =
                readers::pdf::plan_pdf_parts_from_bytes(file_path, bytes, pages, &output_dir_str)
                    .map_err(|e| e.to_string())?;

            for (output_path, content) in files {
                backend
                    .files()
                    .write_bytes(WriteBytesRequest {
                        path: BackendPath(output_path),
                        content,
                        mode: WriteMode::Create,
                    })
                    .await
                    .map_err(|e| format!("Failed to write PDF part: {}", e))?;
            }

            return Ok(FileReadOutput::Parts(parts));
        }

        let pdf_output = readers::pdf::read_pdf_from_bytes(file_path, bytes);
        Ok(FileReadOutput::Pdf(pdf_output))
    }

    async fn process_text(
        &self,
        file_path: &str,
        _ext: &str,
        bytes: &[u8],
        input: &FileReadInput,
        max_tokens: usize,
        mtime: i64,
    ) -> Result<FileReadOutput, String> {
        let text_output =
            readers::text::read_text_from_bytes(file_path, bytes, input.offset, input.limit)
                .map_err(|e| e.to_string())?;

        let estimated_tokens = super::tokenizer::estimate_tokens(file_path, &text_output.content);
        if estimated_tokens > max_tokens {
            return Err(format!(
                "File content token count ({}) exceeds maximum ({})",
                estimated_tokens, max_tokens
            ));
        }

        let mut dedup_store = self.get_dedup_store().await;
        dedup_store.set_read_state(
            file_path.to_string(),
            FileReadState {
                timestamp: mtime,
                offset: input.offset,
                limit: input.limit,
                is_partial_view: input.offset.is_some() || input.limit.is_some(),
            },
        );

        Ok(FileReadOutput::Text(text_output))
    }

    async fn check_dedup_unchanged(
        &self,
        file_path: &str,
        input: &FileReadInput,
        mtime: i64,
    ) -> Option<FileReadOutput> {
        let dedup_store = self.get_dedup_store().await;

        if let Some(state) = dedup_store.get_read_state(file_path) {
            if state.offset.is_some() {
                let is_unchanged = dedup_store.is_file_unchanged(
                    file_path,
                    mtime,
                    input.offset,
                    input.limit,
                    input.offset.is_some() || input.limit.is_some(),
                );

                if is_unchanged {
                    return Some(FileReadOutput::FileUnchanged(
                        super::output::FileUnchangedOutput {
                            file_path: input.file_path.clone(),
                        },
                    ));
                }
            }
        }
        None
    }
}

impl Default for FileReadExecutor {
    fn default() -> Self {
        Self::new(
            Arc::new(FileReadToolSpec::new()),
            ToolRuntimeServices::default(),
        )
    }
}

#[async_trait]
impl ToolExecutor for FileReadExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: FileReadInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "file_read requires operation backend access, but none is configured"
                        .to_string(),
                },
            });
        }
        let backend = backend.unwrap();

        let file_path = Self::normalize_path(&input.file_path);

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

        let resolved_str = resolved.to_string();

        let validation_result =
            validation::validate_input_with_base_from_bytes(&input, &resolved_str);
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

        let stat = backend.files().stat(&resolved).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to stat file: {}", e),
            }
        })?;

        if !stat.exists {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("File does not exist: {}", resolved_str),
                },
            });
        }

        let bytes = backend
            .files()
            .read_bytes(ReadBytesRequest {
                path: resolved.clone(),
            })
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to read file: {}", e),
            })?;

        let ext = Self::get_extension(&resolved_str).unwrap_or_default();
        let mtime = system_time_to_timestamp(stat.modified_at);

        if Self::dedup_applies(&ext) {
            if let Some(output) = self
                .check_dedup_unchanged(&resolved_str, &input, mtime)
                .await
            {
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
        }

        let result = if Self::is_notebook_extension(&ext) {
            self.process_notebook(
                &resolved_str,
                &ext,
                &bytes,
                &input,
                DEFAULT_MAX_SIZE_BYTES,
                DEFAULT_MAX_TOKENS,
                mtime,
            )
            .await
        } else if Self::is_image_extension(&ext) {
            Self::process_image(&resolved_str, &ext, &bytes, DEFAULT_MAX_TOKENS)
        } else if Self::is_pdf_extension(&ext) {
            self.process_pdf(&resolved_str, &ext, &bytes, &input, backend)
                .await
        } else {
            self.process_text(
                &resolved_str,
                &ext,
                &bytes,
                &input,
                DEFAULT_MAX_TOKENS,
                mtime,
            )
            .await
        };

        match result {
            Ok(output) => {
                if let FileReadOutput::Text(_) = &output {
                    if let Some(lsp) = &self.services.lsp_service {
                        spawn_touch_file(lsp, Path::new(&resolved_str));
                    }
                }
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
