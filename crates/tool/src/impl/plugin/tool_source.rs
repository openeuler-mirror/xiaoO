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
    home_dir: Option<PathBuf>,
}

impl PluginToolSource {
    /// Creates a new plugin tool source.
    pub fn new(workspace_root: Option<PathBuf>) -> Self {
        Self::with_home(workspace_root, std::env::var_os("HOME").map(PathBuf::from))
    }

    /// Creates a plugin tool source with an explicit home directory.
    pub fn with_home(workspace_root: Option<PathBuf>, home_dir: Option<PathBuf>) -> Self {
        Self {
            workspace_root,
            home_dir,
        }
    }

    fn discovery_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        let workspace_root = self
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok());
        if let Some(workspace_root) = workspace_root {
            dirs.push(workspace_root.join(".xiaoo").join("tools"));
        }

        if let Some(home) = &self.home_dir {
            let home_tools = home.join(".xiaoo").join("tools");
            if !dirs.contains(&home_tools) {
                dirs.push(home_tools);
            }
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
        let mut discovered_tools = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for dir in self.discovery_dirs() {
            for loaded in Self::discover_dir(&dir) {
                let tool_name = loaded.manifest.name.clone();
                if seen_names.insert(tool_name) {
                    discovered_tools.push(Self::discovered_tool(loaded));
                }
            }
        }

        discovered_tools
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

        let source = PluginToolSource::with_home(Some(workspace.clone()), None);
        let tools = source.discover();
        let echo_tool = tools
            .iter()
            .find(|t| t.spec.name().0 == "echo_payload")
            .expect("echo_payload tool should be discovered");

        let runtime = TestRuntime::new(workspace);
        let output = echo_tool
            .executor
            .invoke(
                &FinalToolCall {
                    call_id: "call-1".to_string(),
                    tool_name: "echo_payload".to_string(),
                    input: json!({"message": "hello"}),
                    ..Default::default()
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

    #[test]
    fn duplicate_tool_names_are_deduplicated() {
        let temp = tempfile::tempdir().expect("tempdir");

        let home_dir = temp.path().join("home");
        let home_tools_dir = home_dir.join(".xiaoo").join("tools");
        std::fs::create_dir_all(&home_tools_dir).expect("home tools dir");

        let workspace = temp.path().join("workspace");
        let workspace_tools_dir = workspace.join(".xiaoo").join("tools");
        std::fs::create_dir_all(&workspace_tools_dir).expect("workspace tools dir");

        std::fs::write(
            home_tools_dir.join("test_tool.toml"),
            r#"
name = "test_tool"
description = "Home version"
timeout_ms = 5000

[input_schema]
type = "object"

[exec]
command = "echo"
args = ["home"]
"#,
        )
        .expect("home manifest");

        std::fs::write(
            workspace_tools_dir.join("test_tool.toml"),
            r#"
name = "test_tool"
description = "Workspace version"
timeout_ms = 5000

[input_schema]
type = "object"

[exec]
command = "echo"
args = ["workspace"]
"#,
        )
        .expect("workspace manifest");

        let source = PluginToolSource::with_home(Some(workspace.clone()), Some(home_dir.clone()));
        let tools = source.discover();

        assert_eq!(
            tools.len(),
            1,
            "Should only have one tool after deduplication"
        );
        assert_eq!(tools[0].spec.name().0, "test_tool");
        assert_eq!(
            tools[0].spec.description(),
            "Workspace version",
            "Workspace tools should have priority over home tools"
        );
    }

    #[test]
    fn same_directory_is_not_scanned_twice() {
        let temp = tempfile::tempdir().expect("tempdir");

        let dir = temp.path().join("same");
        let tools_dir = dir.join(".xiaoo").join("tools");
        std::fs::create_dir_all(&tools_dir).expect("tools dir");

        std::fs::write(
            tools_dir.join("unique_tool.toml"),
            r#"
name = "unique_tool"
description = "Only one instance"
timeout_ms = 5000

[input_schema]
type = "object"

[exec]
command = "echo"
args = ["test"]
"#,
        )
        .expect("manifest");

        let source = PluginToolSource::with_home(Some(dir.clone()), Some(dir.clone()));
        let tools = source.discover();

        assert_eq!(
            tools.len(),
            1,
            "Tool should only be discovered once when workspace == home"
        );
        assert_eq!(tools[0].spec.name().0, "unique_tool");
    }
}
