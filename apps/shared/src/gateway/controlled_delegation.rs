use std::sync::Arc;

use agent_contracts::{
    runtime::runtime_view::RuntimeView,
    tool::{
        DiscoveredTool, ToolExecutor, ToolRegistry, ToolRegistryBuilder, ToolSource, ToolSpecView,
    },
};
use agent_types::{
    common::ids::AgentId,
    tool::{
        FinalToolCall, RawToolOutcome, ToolExecutionError, ToolExecutorOutput, ToolRegistryConfig,
        ToolVisibilityConfig,
    },
};
use async_trait::async_trait;
use serde_json::Value;
use tool::{BuiltinToolSource, ToolRegistryBuilderImpl};

pub use agent_contracts::backend::{
    capability::exec::{ExecOutput, ExecResult},
    OperationError,
};
pub use operation_backend::{ClientCommand, ClientOperationCapabilities, ClientOperations};

#[async_trait]
pub trait ControlledToolClient: ClientOperations {
    async fn start_tool(&self, name: &str, input: &Value) -> Result<String, OperationError>;
    fn finish_tool(&self, id: &str, name: &str, output: Option<&str>);
}

struct ControlledBuiltinSource(Vec<DiscoveredTool>);

impl ToolSource for ControlledBuiltinSource {
    fn discover(&self) -> Vec<DiscoveredTool> {
        self.0
            .iter()
            .map(|tool| DiscoveredTool {
                spec: tool.spec.clone(),
                executor: tool.executor.clone(),
            })
            .collect()
    }
}

fn allows(capabilities: ClientOperationCapabilities, name: &str) -> bool {
    match name {
        "bash" | "glob" | "grep" => capabilities.terminal,
        "file_read" => capabilities.terminal && capabilities.read,
        "file_edit" | "file_write" => {
            capabilities.terminal && capabilities.read && capabilities.write
        }
        "todo_write" | "ask_user_question" => true,
        _ => false,
    }
}

pub(crate) fn registry(
    capabilities: ClientOperationCapabilities,
    client: Arc<dyn ControlledToolClient>,
) -> anyhow::Result<Arc<dyn ToolRegistry>> {
    let tools: Vec<_> = BuiltinToolSource::new(Default::default())
        .discover()
        .into_iter()
        .filter(|tool| allows(capabilities, &tool.spec.name().0))
        .map(|tool| DiscoveredTool {
            spec: tool.spec,
            executor: Arc::new(ControlledExecutor {
                inner: tool.executor,
                client: client.clone(),
            }),
        })
        .collect();
    let names = tools.iter().map(|tool| tool.spec.name().clone()).collect();
    let config = ToolRegistryConfig {
        visibility: ToolVisibilityConfig {
            per_agent_allowed_tools: [(AgentId("main".into()), names)].into(),
        },
    };
    Ok(Arc::from(
        ToolRegistryBuilderImpl::new()
            .with_sources(vec![Box::new(ControlledBuiltinSource(tools))])
            .with_config(config)
            .build()?,
    ))
}

struct ControlledExecutor {
    inner: Arc<dyn ToolExecutor>,
    client: Arc<dyn ControlledToolClient>,
}

#[async_trait]
impl ToolExecutor for ControlledExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.inner.spec()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let id = self
            .client
            .start_tool(&call.tool_name, &call.input)
            .await
            .map_err(|error| {
                if runtime
                    .cancel_token()
                    .is_some_and(|token| token.is_cancelled())
                {
                    ToolExecutionError::Cancelled {
                        message: "prompt cancelled".into(),
                    }
                } else {
                    ToolExecutionError::PermissionDenied {
                        message: error.to_string(),
                    }
                }
            })?;
        let mut result = self.inner.invoke(call, runtime).await;
        if let Err(ToolExecutionError::ExecutionFailed { message }) = &mut result {
            bound_output(message);
        }

        if let Ok(ToolExecutorOutput::Completed { raw_outcome }) = &mut result {
            let text = match raw_outcome {
                RawToolOutcome::Success { output } => output,
                RawToolOutcome::Error { message } => message,
            };
            bound_output(text);
        }
        let output = match &result {
            Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success { output },
            }) => Some(output.as_str()),
            _ => None,
        };
        self.client.finish_tool(&id, &call.tool_name, output);
        result
    }
}

// Keep results below the runtime's file-backed truncation threshold.
fn bound_output(text: &mut String) {
    const BYTES: usize = 32 * 1024;
    const LINES: usize = 1_500;
    if text.len() <= BYTES && text.lines().count() <= LINES {
        return;
    }
    let mut end = BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some((index, _)) = text[..end].match_indices('\n').nth(LINES - 1) {
        end = index;
    }
    text.truncate(end);
    text.push_str("\n[Client tool output truncated. Request a narrower result.]");
}
