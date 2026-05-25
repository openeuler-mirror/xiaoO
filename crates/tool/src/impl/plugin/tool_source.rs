//! Plugin tool sources.

use agent_contracts::tool::{DiscoveredTool, ToolSource};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::executor::DeclarativeToolExecutor;
use super::manifest::LoadedDeclarativeTool;
use super::spec::DeclarativeToolSpec;

/// A plugin tool source.
pub struct PluginToolSource {
    workspace_root: Option<PathBuf>,
}

impl PluginToolSource {
    /// Creates a new plugin tool source.
    pub fn new(workspace_root: Option<PathBuf>) -> Self {
        Self { workspace_root }
    }

    fn discovery_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".xiaoo").join("tools"));
        }

        let workspace_root = self
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok());
        if let Some(workspace_root) = workspace_root {
            dirs.push(workspace_root.join(".xiaoo").join("tools"));
        }

        dirs
    }

    fn discover_dir(dir: &Path) -> Vec<LoadedDeclarativeTool> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };

        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        paths.sort();

        paths
            .into_iter()
            .filter_map(|path| match LoadedDeclarativeTool::load(&path) {
                Ok(tool) => Some(tool),
                Err(error) => {
                    tracing::warn!(error = %error, "failed to load declarative custom tool");
                    None
                }
            })
            .collect()
    }

    fn discovered_tool(loaded: LoadedDeclarativeTool) -> DiscoveredTool {
        let spec = Arc::new(DeclarativeToolSpec::from_loaded_tool(&loaded));
        let executor = DeclarativeToolExecutor::from_loaded_tool(Arc::clone(&spec), &loaded);
        DiscoveredTool {
            spec,
            executor: Arc::new(executor),
        }
    }
}

impl ToolSource for PluginToolSource {
    fn discover(&self) -> Vec<DiscoveredTool> {
        self.discovery_dirs()
            .into_iter()
            .flat_map(|dir| Self::discover_dir(&dir))
            .map(Self::discovered_tool)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_contracts::events::tool_events::ToolEventSink;
    use agent_contracts::hook::registry::HookerRegistry;
    use agent_contracts::interaction::handle::InteractionHandle;
    use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
    use agent_contracts::runtime::runtime_view::RuntimeView;
    use agent_contracts::tool::state::ToolStateStore;
    use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
    use agent_types::common::{AgentMetadata, HookerId, WorkspaceRef};
    use agent_types::events::ToolLifecycleEvent;
    use agent_types::hook::HookPointId;
    use agent_types::interaction::{InteractionRequest, InteractionResponse};
    use agent_types::tool::{
        FinalToolCall, ToolExecutionError, ToolExecutionResult, ToolExecutorOutput,
        ToolLifecycleRecord, ToolLifecycleStatus,
    };
    use agent_types::ChatMessage;
    use async_trait::async_trait;
    use serde_json::json;
    use std::borrow::Cow;

    struct TestConversation;

    impl ConversationView for TestConversation {
        fn recent_messages(&self, _limit: usize) -> Vec<ChatMessage> {
            Vec::new()
        }

        fn message_count(&self) -> usize {
            0
        }
    }

    struct TestAgentContext {
        conversation: TestConversation,
        workspace: WorkspaceRef,
        metadata: AgentMetadata,
    }

    impl TestAgentContext {
        fn new(workspace_root: PathBuf) -> Self {
            Self {
                conversation: TestConversation,
                workspace: WorkspaceRef {
                    root: workspace_root,
                },
                metadata: AgentMetadata {
                    agent_id: "test-agent".to_string(),
                    model: "test-model".to_string(),
                    session_id: Some("session-1".to_string()),
                },
            }
        }
    }

    impl AgentContext for TestAgentContext {
        fn conversation(&self) -> &dyn ConversationView {
            &self.conversation
        }

        fn workspace(&self) -> &WorkspaceRef {
            &self.workspace
        }

        fn metadata(&self) -> &AgentMetadata {
            &self.metadata
        }
    }

    struct NoopToolStateStore;

    impl ToolStateStore for NoopToolStateStore {
        fn begin(
            &self,
            call: &FinalToolCall,
            _spec: &dyn agent_contracts::tool::ToolSpecView,
        ) -> ToolLifecycleRecord {
            ToolLifecycleRecord {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                status: ToolLifecycleStatus::Running,
                started_at_ms: 0,
                finished_at_ms: None,
            }
        }

        fn update(&self, _record: &ToolLifecycleRecord) {}
        fn finish(&self, _record: &ToolLifecycleRecord, _result: &ToolExecutionResult) {}
        fn fail(&self, _record: &ToolLifecycleRecord, _error: &ToolExecutionError) {}
    }

    struct NoopToolEvents;

    impl ToolEventSink for NoopToolEvents {
        fn emit(&self, _event: ToolLifecycleEvent) {}
    }

    struct NoopTraceRecorder;

    #[async_trait]
    impl TraceRecorder for NoopTraceRecorder {
        async fn begin_span(
            &self,
            _kind: TraceSpanKind,
            _name: Cow<'static, str>,
            _fields: serde_json::Value,
        ) -> TraceSpanHandle {
            TraceSpanHandle::new("trace", "span", None)
        }

        async fn update_span(&self, _span: &TraceSpanHandle, _fields: serde_json::Value) {}

        async fn end_span(
            &self,
            _span: TraceSpanHandle,
            _outcome: TraceOutcome,
            _fields: serde_json::Value,
        ) {
        }

        async fn finalize_trace(&self, _outcome: TraceOutcome, _fields: serde_json::Value) {}

        async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: serde_json::Value) {}
    }

    struct NoopInteraction;

    #[async_trait]
    impl InteractionHandle for NoopInteraction {
        async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
            InteractionResponse::Confirmed { allowed: false }
        }
    }

    struct NoopHookers;

    impl HookerRegistry for NoopHookers {
        fn get(&self, _id: &HookerId) -> Option<&dyn agent_contracts::Hooker> {
            None
        }

        fn list(&self) -> Vec<&dyn agent_contracts::Hooker> {
            Vec::new()
        }

        fn list_for_hook_point(
            &self,
            _hook_point: &HookPointId,
        ) -> Vec<&dyn agent_contracts::Hooker> {
            Vec::new()
        }

        fn is_enabled(&self, _id: &HookerId) -> bool {
            false
        }

        fn policy_for(&self, _id: &HookerId) -> Option<&serde_json::Value> {
            None
        }
    }

    struct TestRuntime {
        state_store: NoopToolStateStore,
        tool_events: NoopToolEvents,
        trace_recorder: NoopTraceRecorder,
        agent_context: TestAgentContext,
        interaction: NoopInteraction,
        hookers: NoopHookers,
    }

    impl TestRuntime {
        fn new(workspace_root: PathBuf) -> Self {
            Self {
                state_store: NoopToolStateStore,
                tool_events: NoopToolEvents,
                trace_recorder: NoopTraceRecorder,
                agent_context: TestAgentContext::new(workspace_root),
                interaction: NoopInteraction,
                hookers: NoopHookers,
            }
        }
    }

    impl RuntimeView for TestRuntime {
        fn state_store(&self) -> &dyn ToolStateStore {
            &self.state_store
        }

        fn tool_events(&self) -> &dyn ToolEventSink {
            &self.tool_events
        }

        fn trace_recorder(&self) -> &dyn TraceRecorder {
            &self.trace_recorder
        }

        fn agent_context(&self) -> &dyn AgentContext {
            &self.agent_context
        }

        fn interaction(&self) -> &dyn InteractionHandle {
            &self.interaction
        }

        fn hookers(&self) -> &dyn HookerRegistry {
            &self.hookers
        }
    }

    #[tokio::test]
    async fn declarative_tool_is_discovered_and_invoked() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let tools_dir = workspace.join(".xiaoo").join("tools");
        std::fs::create_dir_all(&tools_dir).expect("tools dir");
        std::fs::write(
            tools_dir.join("echo_payload.toml"),
            r#"
name = "echo_payload"
description = "Echoes the custom tool stdin payload"
timeout_ms = 5000

[input_schema]
type = "object"
required = ["message"]

[input_schema.properties.message]
type = "string"
description = "Message to echo"

[exec]
command = "sh"
args = [".xiaoo/tools/echo_payload.sh"]
stdin = "json"
stdout = "text"
"#,
        )
        .expect("manifest");
        std::fs::write(tools_dir.join("echo_payload.sh"), "cat\n").expect("script");

        let source = PluginToolSource::new(Some(workspace.clone()));
        let tools = source.discover();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].spec.name().0, "echo_payload");

        let runtime = TestRuntime::new(workspace);
        let output = tools[0]
            .executor
            .invoke(
                &FinalToolCall {
                    call_id: "call-1".to_string(),
                    tool_name: "echo_payload".to_string(),
                    input: json!({"message": "hello"}),
                },
                &runtime,
            )
            .await
            .expect("invoke");

        match output {
            ToolExecutorOutput::Completed {
                raw_outcome: agent_types::tool::RawToolOutcome::Success { output },
            } => {
                let payload: serde_json::Value =
                    serde_json::from_str(&output).expect("json payload echoed");
                assert_eq!(payload["args"]["message"], "hello");
                assert_eq!(payload["context"]["agent_id"], "test-agent");
            }
            other => panic!("unexpected custom tool output: {other:?}"),
        }
    }
}
