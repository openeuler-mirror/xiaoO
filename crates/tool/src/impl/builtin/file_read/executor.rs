//! FileReadTool executor implementation.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::constants::{
    DEFAULT_MAX_SIZE_BYTES, DEFAULT_MAX_TOKENS, IMAGE_EXTENSIONS, NOTEBOOK_EXTENSION, PDF_EXTENSION,
};
use super::dedup::{get_file_mtime, DedupStateStore, FileReadState};
use super::input::FileReadInput;
use super::output::FileReadOutput;
use super::readers;
use super::spec::FileReadToolSpec;
use super::validation;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;

/// Executor for FileReadTool.
pub struct FileReadExecutor {
    spec: Arc<FileReadToolSpec>,
    dedup_store: Mutex<DedupStateStore>,
}

impl FileReadExecutor {
    /// Creates a new FileReadExecutor.
    pub fn new(spec: Arc<FileReadToolSpec>) -> Self {
        Self {
            spec,
            dedup_store: Mutex::new(DedupStateStore::new()),
        }
    }

    /// Normalizes a file path (trims whitespace, converts to string).
    fn normalize_path(file_path: &str) -> String {
        file_path.trim().to_string()
    }

    /// Extracts the file extension from a path (lowercase, without leading dot).
    fn get_extension(file_path: &str) -> Option<String> {
        Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
    }

    /// Checks if the extension represents a PDF file.
    fn is_pdf_extension(ext: &str) -> bool {
        ext.eq_ignore_ascii_case(PDF_EXTENSION)
    }

    /// Checks if the extension represents an image file.
    fn is_image_extension(ext: &str) -> bool {
        IMAGE_EXTENSIONS
            .iter()
            .any(|&e| e.eq_ignore_ascii_case(ext))
    }

    /// Checks if the extension represents a notebook file.
    fn is_notebook_extension(ext: &str) -> bool {
        ext.eq_ignore_ascii_case(NOTEBOOK_EXTENSION)
    }

    /// Checks if dedup should apply (only for text/notebook, not image/pdf).
    fn dedup_applies(ext: &str) -> bool {
        !Self::is_image_extension(ext) && !Self::is_pdf_extension(ext)
    }

    async fn get_dedup_store(&self) -> tokio::sync::MutexGuard<'_, DedupStateStore> {
        self.dedup_store.lock().await
    }

    /// Executes the file read operation (callInner in TypeScript).
    async fn call_inner(
        &self,
        input: &FileReadInput,
        full_file_path: &str,
        resolved_file_path: &str,
        ext: &str,
        max_size_bytes: u64,
        max_tokens: usize,
    ) -> Result<FileReadOutput, String> {
        if Self::is_notebook_extension(ext) {
            let cells = readers::notebook::read_notebook(resolved_file_path, Some(max_size_bytes))
                .await
                .map_err(|e| e.to_string())?;

            let cells_json = serde_json::to_string(&cells.cells).map_err(|e| e.to_string())?;
            let cells_json_bytes = cells_json.len() as u64;

            if cells_json_bytes > max_size_bytes {
                return Err(format!(
                    "Notebook content ({} bytes) exceeds maximum allowed size ({} bytes).",
                    cells_json_bytes, max_size_bytes
                ));
            }

            let estimated_tokens = super::tokenizer::estimate_tokens(full_file_path, &cells_json);
            if estimated_tokens > max_tokens {
                return Err(format!(
                    "Notebook content token count ({}) exceeds maximum ({})",
                    estimated_tokens, max_tokens
                ));
            }

            let mtime = get_file_mtime(Path::new(resolved_file_path)).unwrap_or(0);

            let mut dedup_store = self.get_dedup_store().await;
            dedup_store.set_read_state(
                full_file_path.to_string(),
                FileReadState {
                    timestamp: mtime,
                    offset: input.offset,
                    limit: input.limit,
                    is_partial_view: input.offset.is_some() || input.limit.is_some(),
                },
            );

            return Ok(FileReadOutput::Notebook(cells));
        }

        if Self::is_image_extension(ext) {
            let image_output =
                readers::image::read_image_file(resolved_file_path, Some(max_tokens))
                    .map_err(|e| e.to_string())?;

            return Ok(FileReadOutput::Image(image_output));
        }

        if Self::is_pdf_extension(ext) {
            if let Some(ref pages) = input.pages {
                let output_dir =
                    std::env::temp_dir().join(format!("pdf_pages_{}", std::process::id()));

                let parts = readers::pdf::extract_pdf_pages(
                    resolved_file_path,
                    pages,
                    &output_dir.to_string_lossy(),
                )
                .await
                .map_err(|e| e.to_string())?;

                return Ok(FileReadOutput::Parts(parts));
            }

            let pdf_output = readers::pdf::read_pdf(resolved_file_path)
                .await
                .map_err(|e| e.to_string())?;

            return Ok(FileReadOutput::Pdf(pdf_output));
        }

        let text_output =
            readers::text::read_text_file(resolved_file_path, input.offset, input.limit)
                .await
                .map_err(|e| e.to_string())?;

        let estimated_tokens =
            super::tokenizer::estimate_tokens(full_file_path, &text_output.content);
        if estimated_tokens > max_tokens {
            return Err(format!(
                "File content token count ({}) exceeds maximum ({})",
                estimated_tokens, max_tokens
            ));
        }

        let mtime = get_file_mtime(Path::new(resolved_file_path)).unwrap_or(0);

        let mut dedup_store = self.get_dedup_store().await;
        dedup_store.set_read_state(
            full_file_path.to_string(),
            FileReadState {
                timestamp: mtime,
                offset: input.offset,
                limit: input.limit,
                is_partial_view: input.offset.is_some() || input.limit.is_some(),
            },
        );

        Ok(FileReadOutput::Text(text_output))
    }
}

impl Default for FileReadExecutor {
    fn default() -> Self {
        Self::new(Arc::new(FileReadToolSpec::new()))
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
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: FileReadInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let validation_result = validation::validate_input(&input);
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

        let full_file_path = Self::normalize_path(&input.file_path);
        let resolved_file_path = &full_file_path;

        let ext = Self::get_extension(&full_file_path).unwrap_or_default();

        if Self::dedup_applies(&ext) {
            let dedup_store = self.get_dedup_store().await;

            if let Some(state) = dedup_store.get_read_state(&full_file_path) {
                if state.offset.is_some() {
                    let current_mtime = get_file_mtime(Path::new(&full_file_path)).unwrap_or(0);

                    let is_unchanged = dedup_store.is_file_unchanged(
                        &full_file_path,
                        current_mtime,
                        input.offset,
                        input.limit,
                        input.offset.is_some() || input.limit.is_some(),
                    );

                    if is_unchanged {
                        let output =
                            FileReadOutput::FileUnchanged(super::output::FileUnchangedOutput {
                                file_path: input.file_path.clone(),
                            });
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
            }
        }

        match self
            .call_inner(
                &input,
                &full_file_path,
                resolved_file_path,
                &ext,
                DEFAULT_MAX_SIZE_BYTES,
                DEFAULT_MAX_TOKENS,
            )
            .await
        {
            Ok(output) => {
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
