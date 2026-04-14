use std::sync::Arc;

use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::common::ids::AgentId;
use agent_types::tool::{FinalToolCall, RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use subagent::{JoinSubagentRequest, JoinSubagentResult};

use crate::r#impl::ToolRuntimeServices;

use super::input::JoinSubagentInput;
use super::output::JoinSubagentOutput;
use super::spec::JoinSubagentToolSpec;
use super::validation;

pub struct JoinSubagentExecutor {
    spec: Arc<JoinSubagentToolSpec>,
    services: ToolRuntimeServices,
}

impl JoinSubagentExecutor {
    pub fn new(spec: Arc<JoinSubagentToolSpec>, services: ToolRuntimeServices) -> Self {
        Self { spec, services }
    }
}

#[async_trait]
impl ToolExecutor for JoinSubagentExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: JoinSubagentInput =
            serde_json::from_value(call.input.clone()).map_err(|error| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("Failed to parse input: {}", error),
                }
            })?;

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

        let subagent_control = self.services.subagent_control.clone().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed {
                message: "subagent control is not configured".to_string(),
            }
        })?;
        let session_id = runtime
            .agent_context()
            .metadata()
            .session_id
            .clone()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                message: "join_subagent requires a session_id in runtime metadata".to_string(),
            })?;
        let waiter_agent_id = AgentId(runtime.agent_context().metadata().agent_id.clone());

        let result = subagent_control
            .join(JoinSubagentRequest {
                session_id,
                waiter_agent_id,
                target_agent_id: input.target_agent_id,
            })
            .await
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: error.to_string(),
            })?;

        match result {
            JoinSubagentResult::Ready { terminal } => {
                let output =
                    serde_json::to_string(&JoinSubagentOutput { terminal }).map_err(|error| {
                        ToolExecutionError::ExecutionFailed {
                            message: format!("Failed to serialize join_subagent output: {}", error),
                        }
                    })?;
                Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Success { output },
                })
            }
            JoinSubagentResult::Pending { join_id } => Ok(ToolExecutorOutput::Suspended {
                suspend_token: join_id,
            }),
        }
    }
}
