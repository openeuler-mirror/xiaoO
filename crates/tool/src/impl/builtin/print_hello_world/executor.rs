use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use std::sync::Arc;

use super::spec::PrintHelloWorldToolSpec;

pub struct PrintHelloWorldToolExecutor {
    spec: Arc<PrintHelloWorldToolSpec>,
}

impl PrintHelloWorldToolExecutor {
    pub fn new(spec: Arc<PrintHelloWorldToolSpec>) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl ToolExecutor for PrintHelloWorldToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        _call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success {
                output: "Hello, World!".to_string(),
            },
        })
    }
}
