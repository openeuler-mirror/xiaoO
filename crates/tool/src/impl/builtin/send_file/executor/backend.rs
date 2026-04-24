use std::io::Write;
use std::sync::Arc;

use agent_contracts::backend::capability::export::ExportFileRequest;
use agent_contracts::backend::capability::path::{ResolveBase, ResolvePathRequest};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::interaction::InteractionRequest;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use tempfile::NamedTempFile;

use super::super::input::SendFileInput;
use super::super::spec::SendFileToolSpec;

pub struct SendFileToolExecutor {
    spec: Arc<SendFileToolSpec>,
}

impl SendFileToolExecutor {
    pub fn new(spec: Arc<SendFileToolSpec>) -> Self {
        Self { spec }
    }
}

const KIBIBYTE_BYTES: u64 = 1024;
const MEBIBYTE_BYTES: u64 = KIBIBYTE_BYTES * 1024;

fn format_file_size(size_bytes: u64) -> String {
    if size_bytes >= MEBIBYTE_BYTES {
        format!("{:.1}MB", size_bytes as f64 / (MEBIBYTE_BYTES as f64))
    } else if size_bytes >= KIBIBYTE_BYTES {
        format!("{:.1}KB", size_bytes as f64 / (KIBIBYTE_BYTES as f64))
    } else {
        format!("{size_bytes}B")
    }
}

#[async_trait]
impl ToolExecutor for SendFileToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        // 1. Parse input.
        let input: SendFileInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("failed to parse send_file input: {e}"),
            }
        })?;

        let file_sender = match runtime.channel_file_sender() {
            Some(sender) => sender,
            None => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: "send_file is only available in channel sessions (e.g. Feishu)."
                            .to_string(),
                    },
                });
            }
        };

        let operation_backend = match runtime.operation_backend() {
            Some(backend) => backend,
            None => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message:
                            "send_file requires operation backend access, but none is configured"
                                .to_string(),
                    },
                });
            }
        };

        let resolved_path = match operation_backend
            .paths()
            .resolve_path(ResolvePathRequest {
                raw_path: input.file_path.trim().to_string(),
                base: ResolveBase::WorkspaceRoot,
            })
            .await
        {
            Ok(path) => path,
            Err(error) => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!(
                            "Failed to resolve file path \"{}\": {error}",
                            input.file_path
                        ),
                    },
                });
            }
        };

        let exported_file = match operation_backend
            .export()
            .export_file(ExportFileRequest {
                path: resolved_path,
                preferred_name: None,
            })
            .await
        {
            Ok(file) => file,
            Err(error) => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!("Failed to export file \"{}\": {error}", input.file_path),
                    },
                });
            }
        };

        let metadata = exported_file.metadata().clone();
        let file_name = metadata.file_name.as_str();
        let file_size = metadata
            .size_bytes
            .map(format_file_size)
            .unwrap_or_else(|| "unknown size".to_string());
        let confirm_prompt = format!("确认发送文件 \"{}\" ({}) 给您？", file_name, file_size);
        let confirm_request = InteractionRequest::Confirm {
            prompt: confirm_prompt,
            source: None,
        };
        let response = runtime.interaction().ask(&confirm_request).await;
        let confirmed = matches!(
            response,
            agent_types::interaction::InteractionResponse::Confirmed { allowed: true }
        );

        if !confirmed {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success {
                    output: "User cancelled file sending.".to_string(),
                },
            });
        }

        let mut reader = match exported_file.open_read().await {
            Ok(reader) => reader,
            Err(error) => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!(
                            "Failed to read exported file \"{}\": {error}",
                            input.file_path
                        ),
                    },
                });
            }
        };

        let mut temp_file = match NamedTempFile::new() {
            Ok(file) => file,
            Err(error) => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!("Failed to create temp file for send_file: {error}"),
                    },
                });
            }
        };

        let mut buffer = Vec::new();
        if let Err(error) = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buffer).await {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("Failed to read exported file stream: {error}"),
                },
            });
        }

        if let Err(error) = temp_file.write_all(buffer.as_slice()) {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("Failed to write temp file for send_file: {error}"),
                },
            });
        }
        if let Err(error) = temp_file.flush() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("Failed to flush temp file for send_file: {error}"),
                },
            });
        }

        let temp_path = match temp_file.path().to_str() {
            Some(path) => path.to_string(),
            None => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!(
                            "Temp file path is not valid UTF-8: {}",
                            temp_file.path().display()
                        ),
                    },
                });
            }
        };

        match file_sender
            .send_file(temp_path.as_str(), input.label.as_deref())
            .await
        {
            Ok(_) => Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success {
                    output: format!("File \"{}\" sent successfully.", file_name),
                },
            }),
            Err(error) => Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("Failed to send file: {error}"),
                },
            }),
        }
    }
}
