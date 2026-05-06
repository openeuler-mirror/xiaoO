use agent_contracts::backend::{
    OperationBackend, OperationBackendBuildInput, OperationBackendBuilder,
};
use agent_contracts::trace::TraceRecorderBuilder;
use agent_contracts::{
    HookerRegistry, HookerRegistryBuilder, RuntimeView, ToolEventSink, ToolStateStore,
    ToolStateStoreBuilder, TraceRecorder,
};
use agent_types::common::BuildError;
use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};
use agent_types::tool::ToolStateStoreConfig;
use hook::framework::HookerRegistryBuilderImpl;
use operation_backend::OperationBackendBuilderImpl;
use std::sync::Arc;
use tool::ToolStateStoreBuilderImpl;
use trace::TraceRecorderBuilderImpl;

use super::agent_context::MockAgentContext;
use super::interaction::CliInteractionHandle;
use super::tool_events::PrintStdoutToolEventSink;

#[derive(Debug, Clone)]
pub struct ToolCliTraceConfig {
    pub backend: String,
    pub db_path: Option<String>,
}

impl Default for ToolCliTraceConfig {
    fn default() -> Self {
        Self {
            backend: "stdout".to_string(),
            db_path: None,
        }
    }
}

pub struct ToolCliRuntime {
    state_store: Box<dyn ToolStateStore>,
    tool_events: PrintStdoutToolEventSink,
    trace_recorder: Box<dyn TraceRecorder>,
    agent_context: MockAgentContext,
    interaction: CliInteractionHandle,
    hookers: Box<dyn HookerRegistry>,
    operation_backend: Arc<dyn OperationBackend>,
}

impl ToolCliRuntime {
    pub async fn new(trace_config: ToolCliTraceConfig) -> Result<Self, BuildError> {
        let hookers = HookerRegistryBuilderImpl::new()
            .with_config(HookerRegistryConfig {
                default: HookerDefaultMode::None,
                ..HookerRegistryConfig::default()
            })
            .build()?;

        let state_store = ToolStateStoreBuilderImpl::new()
            .with_config(ToolStateStoreConfig {
                backend: serde_json::Value::String("stdout".to_string()),
                retention: serde_json::Value::Null,
            })
            .build()?;

        let trace_recorder = TraceRecorderBuilderImpl::default()
            .from_json(serde_json::json!({
                "storage_backend": trace_config.backend,
                "db_path": trace_config.db_path,
                "agent_id": "tool_cli",
            }))?
            .build()
            .await?;
        let workspace_root =
            std::env::current_dir().map_err(|error| BuildError::DependencyError {
                message: format!("failed to resolve current directory: {error}"),
            })?;
        let operation_backend = OperationBackendBuilderImpl::new()
            .build(&OperationBackendBuildInput {
                workspace_root: Some(workspace_root),
                agent_id: Some("tool_cli".to_string()),
                ..OperationBackendBuildInput::default()
            })
            .await
            .map_err(|error| BuildError::InvalidConfig {
                message: format!("failed to build operation backend: {error}"),
            })?;

        Ok(Self {
            state_store,
            tool_events: PrintStdoutToolEventSink,
            trace_recorder,
            agent_context: MockAgentContext::new(),
            interaction: CliInteractionHandle,
            hookers,
            operation_backend,
        })
    }
}

impl RuntimeView for ToolCliRuntime {
    fn state_store(&self) -> &dyn ToolStateStore {
        self.state_store.as_ref()
    }

    fn tool_events(&self) -> &dyn ToolEventSink {
        &self.tool_events
    }

    fn trace_recorder(&self) -> &dyn TraceRecorder {
        self.trace_recorder.as_ref()
    }

    fn agent_context(&self) -> &dyn agent_contracts::AgentContext {
        &self.agent_context
    }

    fn interaction(&self) -> &dyn agent_contracts::InteractionHandle {
        &self.interaction as &dyn agent_contracts::InteractionHandle
    }

    fn hookers(&self) -> &dyn HookerRegistry {
        self.hookers.as_ref()
    }

    fn operation_backend(&self) -> Option<Arc<dyn OperationBackend>> {
        Some(Arc::clone(&self.operation_backend))
    }
}
