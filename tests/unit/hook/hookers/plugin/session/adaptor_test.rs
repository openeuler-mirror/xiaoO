use super::*;
use std::borrow::Cow;
use std::path::PathBuf;

use super::super::super::test_support::block_on;

use agent_contracts::events::tool_events::ToolEventSink;
use agent_contracts::hook::registry::HookerRegistry;
use agent_contracts::interaction::handle::InteractionHandle;
use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::state::ToolStateStore;
use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
use agent_types::common::{AgentMetadata, HookerId, WorkspaceRef};
use agent_types::events::ToolLifecycleEvent;
use agent_types::hook::{HookInvokePrimary, HookPointId};
use agent_types::tool::execution_types::ToolExecutionError;
use agent_types::tool::FinalToolCall;

struct TestConversation;
impl ConversationView for TestConversation {
    fn recent_messages(&self, _limit: usize) -> Vec<agent_types::ChatMessage> {
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
    fn new() -> Self {
        Self {
            conversation: TestConversation,
            workspace: WorkspaceRef {
                root: PathBuf::from("/tmp"),
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

struct TestInteractionHandle;
#[async_trait]
impl InteractionHandle for TestInteractionHandle {
    async fn ask(
        &self,
        _request: &agent_types::interaction::InteractionRequest,
    ) -> agent_types::interaction::InteractionResponse {
        agent_types::interaction::InteractionResponse::Confirmed { allowed: false }
    }
}

struct TestHookerRegistry;
impl HookerRegistry for TestHookerRegistry {
    fn get(&self, _id: &HookerId) -> Option<&dyn Hooker> {
        None
    }
    fn list(&self) -> Vec<&dyn Hooker> {
        Vec::new()
    }
    fn list_for_hook_point(
        &self,
        _hook_point: &agent_types::hook::HookPointId,
    ) -> Vec<&dyn Hooker> {
        Vec::new()
    }
    fn is_enabled(&self, _id: &HookerId) -> bool {
        false
    }
    fn policy_for(&self, _id: &HookerId) -> Option<&serde_json::Value> {
        None
    }
}

struct TestToolStateStore;
impl ToolStateStore for TestToolStateStore {
    fn begin(
        &self,
        _call: &FinalToolCall,
        _spec: &dyn agent_contracts::tool::spec::ToolSpecView,
    ) -> agent_types::tool::ToolLifecycleRecord {
        panic!("unused in session adaptor test")
    }
    fn update(&self, _record: &agent_types::tool::ToolLifecycleRecord) {}
    fn finish(
        &self,
        _record: &agent_types::tool::ToolLifecycleRecord,
        _result: &agent_types::tool::execution_types::ToolExecutionResult,
    ) {
    }
    fn fail(&self, _record: &agent_types::tool::ToolLifecycleRecord, _error: &ToolExecutionError) {}
}

struct TestToolEventSink;
impl ToolEventSink for TestToolEventSink {
    fn emit(&self, _event: ToolLifecycleEvent) {}
}

struct TestTraceRecorder;
#[async_trait]
impl TraceRecorder for TestTraceRecorder {
    async fn begin_span(
        &self,
        _kind: TraceSpanKind,
        _name: Cow<'static, str>,
        _fields: Value,
    ) -> TraceSpanHandle {
        TraceSpanHandle::new("trace-test", "span-test", None)
    }
    async fn update_span(&self, _span: &TraceSpanHandle, _fields: Value) {}
    async fn end_span(&self, _span: TraceSpanHandle, _outcome: TraceOutcome, _fields: Value) {}
    async fn finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {}
    async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {}
}

struct TestRuntimeView {
    state_store: TestToolStateStore,
    tool_events: TestToolEventSink,
    trace_recorder: TestTraceRecorder,
    agent_context: TestAgentContext,
    interaction: TestInteractionHandle,
    hookers: TestHookerRegistry,
}
impl TestRuntimeView {
    fn new() -> Self {
        Self {
            state_store: TestToolStateStore,
            tool_events: TestToolEventSink,
            trace_recorder: TestTraceRecorder,
            agent_context: TestAgentContext::new(),
            interaction: TestInteractionHandle,
            hookers: TestHookerRegistry,
        }
    }
}
impl RuntimeView for TestRuntimeView {
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

fn adaptor_for(command: &str) -> PluginSessionHookerAdaptor {
    PluginSessionHookerAdaptor::new(
        HookerId("plugin_session_test".to_string()),
        HookPointId("defaultagent.Session.lifecycle.state".to_string()),
        command.to_string(),
        Value::Null,
    )
}

fn idle_input() -> SessionStateHookInput {
    SessionStateHookInput {
        session_id: "s1".to_string(),
        sender_id: "u1".to_string(),
        agent_id: "defaultagent".to_string(),
        state: "idle".to_string(),
        outcome: "complete".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    }
}

/// Write `json` to a temp file and return a `sh -c` command string that
/// drains stdin to `/dev/null` then prints the file. Draining stdin
/// avoids a broken-pipe race with `run_plugin_command`, which always
/// writes a payload to the child's stdin even though these canned-response
/// tests ignore it.
fn cat_command_for(json: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "xiaoo_session_hook_test_{}_{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::write(&path, json).unwrap();
    format!("cat > /dev/null; cat {}", path.display())
}

#[test]
fn session_state_ack_returns_acknowledged() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"ack"}"#));
    let runtime = TestRuntimeView::new();
    let output = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ))
    .unwrap();
    match output.primary {
        HookInvokePrimary::SessionState(SessionHookResult::Acknowledged) => {}
        other => panic!("expected SessionState(Acknowledged), got {:?}", other),
    }
}

#[test]
fn session_state_acknowledged_alias_also_accepted() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"acknowledged"}"#));
    let runtime = TestRuntimeView::new();
    let output = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ))
    .unwrap();
    match output.primary {
        HookInvokePrimary::SessionState(SessionHookResult::Acknowledged) => {}
        other => panic!("expected SessionState(Acknowledged), got {:?}", other),
    }
}

#[test]
fn session_state_transform_result_is_rejected() {
    // Event-style hooks have no mutable output; transform is invalid.
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"transform","system":["x"]}"#));
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(
        result.is_err(),
        "transform should be rejected for state hook"
    );
}

#[test]
fn session_state_missing_result_field_errors() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"foo":"bar"}"#));
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_state_nonzero_exit_errors() {
    // `false` exits with status 1.
    let adaptor = adaptor_for("cat > /dev/null; false");
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_state_invalid_json_errors() {
    let adaptor = adaptor_for("cat > /dev/null; echo not-json");
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_state_payload_carries_state_and_session_fields() {
    // Verify the payload JSON contains the expected fields before the
    // command ignores them. Use a script that echoes the payload back so
    // we can inspect it.
    let adaptor = adaptor_for("cat");
    let runtime = TestRuntimeView::new();
    let input = SessionStateHookInput {
        session_id: "s-idle-1".to_string(),
        sender_id: "u-sender-1".to_string(),
        agent_id: "agent-x".to_string(),
        state: "idle".to_string(),
        outcome: "complete".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    };
    let payload =
        adaptor.build_session_state_payload(&input, &HookInvokeMetadata::default(), &runtime);
    assert_eq!(payload["stage"], json!("session_state"));
    assert_eq!(payload["state"], json!("idle"));
    assert_eq!(payload["outcome"], json!("complete"));
    assert_eq!(payload["session_id"], json!("s-idle-1"));
    assert_eq!(payload["sender_id"], json!("u-sender-1"));
    assert_eq!(payload["agent_id"], json!("agent-x"));
    assert_eq!(payload["hooker"]["id"], json!("plugin_session_test"));
    assert_eq!(
        payload["hooker"]["hook_point"],
        json!("defaultagent.Session.lifecycle.state")
    );
}
