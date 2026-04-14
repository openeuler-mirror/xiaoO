use crate::runtime::runtime_view::RuntimeView;
use crate::tool::registry::ToolFilter;
use agent_types::common::BuildError;
use agent_types::tool::{FinalToolCall, RawToolCall, ToolExecutionError, ToolExecutionResult};
use async_trait::async_trait;

pub trait ToolCallBuilder: Send {
    fn with_raw_llm_tool_call(self, raw_tool_call: RawToolCall) -> Self
    where
        Self: Sized;

    fn with_tool_filter(self, tool_filter: Box<dyn ToolFilter>) -> Self
    where
        Self: Sized;

    fn build(self) -> Result<Box<dyn ToolCall>, BuildError>
    where
        Self: Sized;
}

#[async_trait]
pub trait ToolCall: Send + Sync {
    fn final_call(&self) -> &FinalToolCall;

    async fn execute(
        &self,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutionResult, ToolExecutionError>;
}
