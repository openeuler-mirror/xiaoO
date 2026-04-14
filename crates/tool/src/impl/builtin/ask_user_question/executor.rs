use async_trait::async_trait;
use std::sync::Arc;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::interaction::types::{InteractionRequest, InteractionResponse, InteractionSource};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::input::{AskUserQuestionInput, QuestionItem};
use super::output::{AnswerItem, AskUserQuestionOutput};
use super::spec::AskUserQuestionToolSpec;
use super::validation;

pub struct AskUserQuestionExecutor {
    spec: Arc<AskUserQuestionToolSpec>,
}

impl AskUserQuestionExecutor {
    pub fn new(spec: Arc<AskUserQuestionToolSpec>) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl ToolExecutor for AskUserQuestionExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: AskUserQuestionInput =
            serde_json::from_value(call.input.clone()).map_err(|e| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("Failed to parse input: {}", e),
                }
            })?;

        // 校验输入
        let validation_result = validation::validate_input(&input);
        if !validation_result.result {
            let message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, message),
                },
            });
        }

        let source = Some(InteractionSource::Tool {
            tool_name: "ask_user_question".to_string(),
        });

        let mut answers = Vec::with_capacity(input.questions.len());

        for question in &input.questions {
            let (request, prompt) = match question {
                QuestionItem::Confirm { prompt } => (
                    InteractionRequest::Confirm {
                        prompt: prompt.clone(),
                        source: source.clone(),
                    },
                    prompt.clone(),
                ),
                QuestionItem::TextInput { prompt } => (
                    InteractionRequest::TextInput {
                        prompt: prompt.clone(),
                        source: source.clone(),
                    },
                    prompt.clone(),
                ),
                QuestionItem::Choice {
                    prompt,
                    options,
                    allow_custom_input,
                } => (
                    InteractionRequest::Choice {
                        prompt: prompt.clone(),
                        options: options.clone(),
                        allow_custom_input: *allow_custom_input,
                        source: source.clone(),
                    },
                    prompt.clone(),
                ),
            };

            let response = runtime.interaction().ask(&request).await;

            let answer = match response {
                InteractionResponse::Confirmed { allowed } => {
                    AnswerItem::Confirmed { prompt, allowed }
                }
                InteractionResponse::Text { value } => AnswerItem::Text { prompt, value },
                InteractionResponse::Choice { value } => AnswerItem::Choice { prompt, value },
            };
            answers.push(answer);
        }

        let output = AskUserQuestionOutput { answers };
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
