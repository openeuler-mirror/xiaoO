//! FileWriteTool executor implementation.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use similar::{ChangeTag, TextDiff};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::input::FileWriteInput;
use super::output::{CreateOutput, FileWriteOutput, Hunk, StructuredPatch, UpdateOutput};
use super::spec::FileWriteToolSpec;
use super::validation;
use super::validation::expand_path;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;

/// Executor for FileWriteTool.
pub struct FileWriteExecutor {
    spec: Arc<FileWriteToolSpec>,
}

impl FileWriteExecutor {
    /// Creates a new FileWriteExecutor.
    pub fn new(spec: Arc<FileWriteToolSpec>) -> Self {
        Self { spec }
    }

    /// Normalizes a file path (trims whitespace).
    fn normalize_path(file_path: &str) -> String {
        file_path.trim().to_string()
    }

    /// Generates structured patch from old and new content.
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

    /// Performs atomic file write: write to temp file then rename.
    async fn atomic_write(file_path: &str, content: &str) -> Result<(), String> {
        let path = Path::new(file_path);
        let parent_dir = path
            .parent()
            .ok_or_else(|| format!("Cannot determine parent directory for: {}", file_path))?;

        // Create parent directory if it doesn't exist
        if !parent_dir.as_os_str().is_empty() {
            fs::create_dir_all(parent_dir)
                .await
                .map_err(|e| format!("Failed to create parent directory: {}", e))?;
        }

        // Generate temp file path in the same directory
        let temp_file = parent_dir.join(format!(
            ".tmp_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        // Write content to temp file
        {
            let mut file = fs::File::create(&temp_file)
                .await
                .map_err(|e| format!("Failed to create temp file: {}", e))?;

            file.write_all(content.as_bytes())
                .await
                .map_err(|e| format!("Failed to write to temp file: {}", e))?;

            file.flush()
                .await
                .map_err(|e| format!("Failed to flush temp file: {}", e))?;
        }

        // Atomic rename from temp file to target file
        std::fs::rename(&temp_file, path)
            .map_err(|e| format!("Failed to rename temp file to target: {}", e))?;

        Ok(())
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
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        // Parse input from JSON
        let input: FileWriteInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        // Run validation
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

        let file_path = Self::normalize_path(&input.file_path);
        let expanded_path = expand_path(&file_path);
        let path = Path::new(&expanded_path);

        // Check if file exists and read current content
        let original_content = match fs::read_to_string(&path).await {
            Ok(content) => Some(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!("Failed to read existing file: {}", e),
                    },
                });
            }
        };

        // Generate structured diff
        let structured_patch = match &original_content {
            Some(old) => Self::generate_structured_patch(old, &input.content),
            None => StructuredPatch { hunks: None },
        };

        // Atomic write
        if let Err(e) = Self::atomic_write(&expanded_path, &input.content).await {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("Failed to write file: {}", e),
                },
            });
        }

        // Build output
        let output: FileWriteOutput = match original_content {
            Some(old_content) => FileWriteOutput::Update(UpdateOutput {
                file_path: input.file_path.clone(),
                content: input.content.clone(),
                structured_patch,
                original_file: old_content,
                git_diff: None,
            }),
            None => FileWriteOutput::Create(CreateOutput {
                file_path: input.file_path.clone(),
                content: input.content.clone(),
                structured_patch,
                original_file: serde_json::Value::Null,
                git_diff: None,
            }),
        };

        // Serialize output
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
