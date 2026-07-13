use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;
use std::sync::Arc;

use super::spec::McpToolSpec;

pub struct McpToolExecutor {
    spec: Arc<McpToolSpec>,
    client: Arc<mcp::McpClient>,
    tool_name: String,
}

impl McpToolExecutor {
    pub fn new(spec: Arc<McpToolSpec>, client: Arc<mcp::McpClient>, tool_name: String) -> Self {
        Self {
            spec,
            client,
            tool_name,
        }
    }
}

#[async_trait]
impl ToolExecutor for McpToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let result = self
            .client
            .call_tool(&self.tool_name, call.input.clone())
            .await
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "mcp tool '{}' on server '{}' failed: {error}",
                    self.tool_name,
                    self.client.server_name(),
                ),
            })?;

        let output = result.flatten_text();
        let raw_outcome = if result.is_error {
            RawToolOutcome::Error { message: output }
        } else {
            RawToolOutcome::Success { output }
        };

        Ok(ToolExecutorOutput::Completed { raw_outcome })
    }
}
