use std::sync::Arc;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::interaction::InteractionRequest;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

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

        // 2. Check file exists.
        if !std::path::Path::new(&input.file_path).exists() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("file not found: {}", input.file_path),
                },
            });
        }

        // 3. Check channel_file_sender is available.
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

        // 4. Ask user for confirmation.
        let file_name = std::path::Path::new(&input.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&input.file_path);

        let file_size = std::fs::metadata(&input.file_path)
            .map(|m| {
                let bytes = m.len();
                if bytes >= 1024 * 1024 {
                    format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
                } else if bytes >= 1024 {
                    format!("{:.1}KB", bytes as f64 / 1024.0)
                } else {
                    format!("{}B", bytes)
                }
            })
            .unwrap_or_default();
        let confirm_prompt = format!(
            "\u{786e}\u{8ba4}\u{53d1}\u{9001}\u{6587}\u{4ef6} \"{}\" ({}) \u{7ed9}\u{60a8}\u{ff1f}",
            file_name, file_size
        );
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

        // 5. Send the file.
        match file_sender
            .send_file(&input.file_path, input.label.as_deref())
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
