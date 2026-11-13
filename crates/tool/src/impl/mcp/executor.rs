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
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        // The runtime only carries a token when the turn is cancellable.
        // Passing it down stops this call from waiting for a slow MCP tool
        // after the user cancels, and lets the transport tell the server.
        let cancel = runtime.cancel_token();
        let result = self
            .client
            .call_tool_with_cancel(&self.tool_name, call.input.clone(), cancel.as_ref())
            .await
            .map_err(|error| {
                // Cancellation is a turn-level outcome, not a tool failure:
                // tool_exec drops it from the transcript and the event stream.
                if matches!(error, mcp::McpError::Cancelled) {
                    ToolExecutionError::Cancelled {
                        message: format!(
                            "mcp tool '{}' on server '{}' cancelled",
                            self.tool_name,
                            self.client.server_name(),
                        ),
                    }
                } else {
                    ToolExecutionError::ExecutionFailed {
                        message: format!(
                            "mcp tool '{}' on server '{}' failed: {error}",
                            self.tool_name,
                            self.client.server_name(),
                        ),
                    }
                }
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
