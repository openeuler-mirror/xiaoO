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
use agent_types::chat::ModelRef;
use agent_types::common::HookerId;
use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::events::ToolLifecycleEvent;
use agent_types::hook::{HookInvokePrimary, HookPointId};
use agent_types::tool::execution_types::ToolExecutionError;
use agent_types::tool::FinalToolCall;
use agent_types::{ContentBlock, MessageRole};

fn user_text(text: &str) -> ChatMessage {
    ChatMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    }
}

// ---- minimal test runtime view --------------------------------------

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
    async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
        InteractionResponse::Confirmed { allowed: false }
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
    fn list_for_hook_point(&self, _hook_point: &HookPointId) -> Vec<&dyn Hooker> {
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
        panic!("unused in chat adaptor test")
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

fn adaptor_for(command: &str) -> PluginChatHookerAdaptor {
    PluginChatHookerAdaptor::new(
        HookerId("plugin_chat_test".to_string()),
        HookPointId("test-agent.Chat.system.transform".to_string()),
        command.to_string(),
        Value::Null,
    )
}

/// Write `json` to a temp file and return a `sh -c` command string that
/// drains stdin to `/dev/null` then prints the file. Draining stdin avoids
/// a broken-pipe race with `run_plugin_command`, which always writes a
/// payload to the child's stdin even though these canned-response tests
/// ignore it. A monotonic counter guarantees a unique filename even when
/// two tests allocate one in the same nanosecond (which `SystemTime` could
/// not, causing flaky cross-test collisions).
fn cat_command_for(json: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "xiaoo_chat_hook_test_{}_{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::write(&path, json).unwrap();
    format!("cat > /dev/null; cat {}", path.display())
}

// ---- system transform ----------------------------------------------

#[test]
fn system_transform_allow_keeps_parts() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"allow"}"#));
    let runtime = TestRuntimeView::new();
    let input = ChatSystemTransformInput {
        session_id: Some("s1".to_string()),
        model: ModelRef::default(),
        current_system: vec!["base".to_string()],
    };
    let output =
        block_on(adaptor.invoke_system_transform(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatSystemTransform(ChatSystemTransformResult::Allow) => {}
        other => panic!("expected Allow, got {:?}", other),
    }
}

#[test]
fn system_transform_transform_replaces_parts() {
    let adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"transform","system":["new","parts"]}"#,
    ));
    let runtime = TestRuntimeView::new();
    let input = ChatSystemTransformInput {
        session_id: Some("s1".to_string()),
        model: ModelRef::default(),
        current_system: vec!["base".to_string()],
    };
    let output =
        block_on(adaptor.invoke_system_transform(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatSystemTransform(ChatSystemTransformResult::Transform { system }) => {
            assert_eq!(system, vec!["new".to_string(), "parts".to_string()]);
        }
        other => panic!("expected Transform, got {:?}", other),
    }
}

// ---- chat message ---------------------------------------------------

#[test]
fn chat_message_accept_keeps_message() {
    let mut adaptor = adaptor_for(&cat_command_for(r#"{"result":"accept"}"#));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
    let runtime = TestRuntimeView::new();
    let candidate = user_text("hello");
    let input = ChatMessageHookInput {
        session_id: "s1".to_string(),
        agent: Some("test-agent".to_string()),
        model: None,
        message_id: None,
        message: candidate,
        prior_message_count: 0,
    };
    let output =
        block_on(adaptor.invoke_chat_message(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatMessage(ChatMessageHookResult::Accept) => {}
        other => panic!("expected Accept, got {:?}", other),
    }
}

#[test]
fn chat_message_transform_replaces_message() {
    let mut adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"transform","message":{"role":"user","blocks":[{"type":"text","text":"redacted"}],"timestamp_ms":0,"message_id":null,"api_usage_tokens":null,"reasoning_content":null,"estimated_tokens":null}}"#,
    ));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
    let runtime = TestRuntimeView::new();
    let candidate = user_text("secret sk-abc");
    let input = ChatMessageHookInput {
        session_id: "s1".to_string(),
        agent: Some("test-agent".to_string()),
        model: None,
        message_id: None,
        message: candidate,
        prior_message_count: 0,
    };
    let output =
        block_on(adaptor.invoke_chat_message(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatMessage(ChatMessageHookResult::Transform { message }) => {
            assert_eq!(message.blocks.len(), 1);
            match &message.blocks[0] {
                ContentBlock::Text { text } => assert_eq!(text, "redacted"),
                other => panic!("expected text block, got {:?}", other),
            }
        }
        other => panic!("expected Transform, got {:?}", other),
    }
}

// ---- command before -------------------------------------------------

#[test]
fn command_before_allow_keeps_body() {
    let mut adaptor = adaptor_for(&cat_command_for(r#"{"result":"allow"}"#));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let runtime = TestRuntimeView::new();
    let input = CommandExecuteBeforeInput {
        command: "review".to_string(),
        session_id: "s1".to_string(),
        arguments: "src/main.rs".to_string(),
        body: "Review this carefully.\n\nsrc/main.rs".to_string(),
    };
    let output =
        block_on(adaptor.invoke_command_before(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::CommandExecuteBefore(CommandExecuteBeforeResult::Allow) => {}
        other => panic!("expected Allow, got {:?}", other),
    }
}

#[test]
fn command_before_transform_rewrites_body() {
    let mut adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"transform","body":"rewritten body"}"#,
    ));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let runtime = TestRuntimeView::new();
    let input = CommandExecuteBeforeInput {
        command: "review".to_string(),
        session_id: "s1".to_string(),
        arguments: "".to_string(),
        body: "original".to_string(),
    };
    let output =
        block_on(adaptor.invoke_command_before(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::CommandExecuteBefore(CommandExecuteBeforeResult::Transform { body }) => {
            assert_eq!(body, "rewritten body");
        }
        other => panic!("expected Transform, got {:?}", other),
    }
}

#[test]
fn command_before_deny_carries_reason() {
    let mut adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"deny","reason":"blocked by policy"}"#,
    ));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let runtime = TestRuntimeView::new();
    let input = CommandExecuteBeforeInput {
        command: "deploy".to_string(),
        session_id: "s1".to_string(),
        arguments: "".to_string(),
        body: "deploy prod".to_string(),
    };
    let output =
        block_on(adaptor.invoke_command_before(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::CommandExecuteBefore(CommandExecuteBeforeResult::Deny { reason }) => {
            assert_eq!(reason, "blocked by policy");
        }
        other => panic!("expected Deny, got {:?}", other),
    }
}

// ---- payload workspace field ------------------------------------------

#[test]
fn chat_payloads_carry_workspace_root() {
    let runtime = TestRuntimeView::new();

    let mut adaptor = adaptor_for("cat > /dev/null");

    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let command_input = CommandExecuteBeforeInput {
        command: "review".to_string(),
        session_id: "s1".to_string(),
        arguments: "".to_string(),
        body: "body".to_string(),
    };
    let payload = adaptor.build_command_before_payload(
        &command_input,
        &HookInvokeMetadata::default(),
        &runtime,
    );
    assert_eq!(payload["stage"], json!("command_before"));
    assert_eq!(payload["workspace"], json!("/tmp"));

    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
    let message_input = ChatMessageHookInput {
        session_id: "s1".to_string(),
        agent: None,
        model: None,
        message_id: None,
        message: user_text("hello"),
        prior_message_count: 0,
    };
    let payload = adaptor.build_chat_message_payload(
        &message_input,
        &HookInvokeMetadata::default(),
        &runtime,
    );
    assert_eq!(payload["stage"], json!("chat_message"));
    assert_eq!(payload["workspace"], json!("/tmp"));

    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.system.transform".to_string()));
    let system_input = ChatSystemTransformInput {
        session_id: Some("s1".to_string()),
        model: ModelRef::default(),
        current_system: vec!["base".to_string()],
    };
    let payload = adaptor.build_system_transform_payload(
        &system_input,
        &HookInvokeMetadata::default(),
        &runtime,
    );
    assert_eq!(payload["stage"], json!("system_transform"));
    assert_eq!(payload["workspace"], json!("/tmp"));
}
