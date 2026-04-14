use std::sync::Arc;

use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::{FinalToolCall, RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use subagent::SpawnSubagentRequest;

use crate::r#impl::ToolRuntimeServices;

use super::input::SpawnSubagentInput;
use super::output::SpawnSubagentOutput;
use super::spec::SpawnSubagentToolSpec;
use super::validation;

pub struct SpawnSubagentExecutor {
    spec: Arc<SpawnSubagentToolSpec>,
    services: ToolRuntimeServices,
}

impl SpawnSubagentExecutor {
    pub fn new(spec: Arc<SpawnSubagentToolSpec>, services: ToolRuntimeServices) -> Self {
        Self { spec, services }
    }
}

#[async_trait]
impl ToolExecutor for SpawnSubagentExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: SpawnSubagentInput =
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
                message: "spawn_subagent requires a session_id in runtime metadata".to_string(),
            })?;
        let parent_agent_id =
            agent_types::common::ids::AgentId(runtime.agent_context().metadata().agent_id.clone());

        let result = subagent_control
            .spawn(SpawnSubagentRequest {
                session_id,
                parent_agent_id,
                description: input.description,
                prompt: input.prompt,
            })
            .await
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: error.to_string(),
            })?;

        let output = serde_json::to_string(&SpawnSubagentOutput {
            agent_id: result.agent_id,
        })
        .map_err(|error| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to serialize spawn_subagent output: {}", error),
        })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output },
        })
    }
}
