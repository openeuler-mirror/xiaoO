use crate::runtime::runtime_view::RuntimeView;
use crate::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> &dyn ToolSpecView;

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError>;
}
