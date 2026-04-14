use std::sync::Arc;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::skill::registry::SkillContext;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError};
use async_trait::async_trait;

use super::input::SkillToolInput;
use super::spec::SkillToolSpec;
use super::substitute::substitute_arguments;

pub struct SkillToolExecutor {
    spec: Arc<SkillToolSpec>,
}

impl SkillToolExecutor {
    pub fn new(spec: Arc<SkillToolSpec>) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl ToolExecutor for SkillToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<RawToolOutcome, ToolExecutionError> {
        let input: SkillToolInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("failed to parse skill tool input: {}", e),
            }
        })?;

        // 1. Get skill registry from runtime
        let registry = runtime
            .skill_registry()
            .ok_or(ToolExecutionError::ExecutionFailed {
                message: "skill registry not available".into(),
            })?;

        // 2. Look up skill by name
        let spec = registry
            .get_skill(&input.skill)
            .ok_or(ToolExecutionError::ExecutionFailed {
                message: format!("skill '{}' not found", input.skill),
            })?;

        // 3. Check invocation permission
        if spec.disable_model_invocation() {
            return Ok(RawToolOutcome::Error {
                message: format!(
                    "skill '{}' can only be invoked by user, not by model",
                    input.skill
                ),
            });
        }

        // 4. Substitute arguments in prompt
        let prompt = substitute_arguments(spec.full_prompt(), &input.args, spec.arguments());

        // 5. Execute based on context mode
        match spec.context() {
            SkillContext::Inline => Ok(RawToolOutcome::Success { output: prompt }),
            SkillContext::Fork => {
                // TODO: Fork execution not yet implemented
                Ok(RawToolOutcome::Error {
                    message: "fork context is not yet supported".into(),
                })
            }
        }
    }
}
