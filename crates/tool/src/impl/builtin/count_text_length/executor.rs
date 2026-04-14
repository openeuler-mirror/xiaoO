use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use std::sync::Arc;

use super::spec::CountTextLengthToolSpec;

pub struct CountTextLengthToolExecutor {
    spec: Arc<CountTextLengthToolSpec>,
}

impl CountTextLengthToolExecutor {
    pub fn new(spec: Arc<CountTextLengthToolSpec>) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl ToolExecutor for CountTextLengthToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let text = call
            .input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                message: "Missing or invalid 'text' field: expected a string".to_string(),
            })?;

        let count = text.chars().count();
        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success {
                output: count.to_string(),
            },
        })
    }
}
