use super::*;
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Mutex;

use agent_contracts::events::tool_events::ToolEventSink;
use agent_contracts::hook::registry::HookerRegistry;
use agent_contracts::interaction::handle::InteractionHandle;
use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::state::ToolStateStore;
use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::events::ToolLifecycleEvent;
use agent_types::hook::HookInvokePrimary;
use agent_types::tool::FinalToolCall;
use agent_types::ChatMessage;

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

struct TestInteractionHandle {
    response: InteractionResponse,
    requests: Mutex<Vec<InteractionRequest>>,
}

impl TestInteractionHandle {
    fn new(response: InteractionResponse) -> Self {
        Self {
            response,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn recorded_requests(&self) -> Vec<InteractionRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl InteractionHandle for TestInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        self.requests.lock().unwrap().push(request.clone());
        self.response.clone()
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
        panic!("unused in adaptor test")
    }

    fn update(&self, _record: &agent_types::tool::ToolLifecycleRecord) {
        panic!("unused in adaptor test")
    }

    fn finish(
        &self,
        _record: &agent_types::tool::ToolLifecycleRecord,
        _result: &agent_types::tool::execution_types::ToolExecutionResult,
    ) {
        panic!("unused in adaptor test")
    }

    fn fail(&self, _record: &agent_types::tool::ToolLifecycleRecord, _error: &ToolExecutionError) {
        panic!("unused in adaptor test")
    }
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
        panic!("unused in adaptor test")
    }

    async fn update_span(&self, _span: &TraceSpanHandle, _fields: Value) {
        panic!("unused in adaptor test")
    }

    async fn end_span(&self, _span: TraceSpanHandle, _outcome: TraceOutcome, _fields: Value) {
        panic!("unused in adaptor test")
    }

    async fn finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {
        panic!("unused in adaptor test")
    }

    async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {
        panic!("unused in adaptor test")
    }
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
    fn new(response: InteractionResponse) -> Self {
        Self {
            state_store: TestToolStateStore,
            tool_events: TestToolEventSink,
            trace_recorder: TestTraceRecorder,
            agent_context: TestAgentContext::new(),
            interaction: TestInteractionHandle::new(response),
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

#[tokio::test]
async fn ask_user_round_trip_returns_final_pre_result() {
    let script_path = std::env::temp_dir().join(format!(
        "plugin_ask_user_round_trip_{}.py",
        std::process::id()
    ));
    std::fs::write(
        &script_path,
        r#"import json
import sys

payload = json.load(sys.stdin)
interaction = payload.get("interaction")
if interaction is None:
    json.dump(
        {
            "action": "ask_user",
            "request": {
                "kind": "text_input",
                "prompt": "who approved this?",
            },
            "continuation": {"step": "final"},
        },
        sys.stdout,
    )
else:
    value = interaction["response"].get("value")
    json.dump(
        {
            "action": "final",
            "result": "deny",
            "reason": f"approved by {value}",
        },
        sys.stdout,
    )
"#,
    )
    .unwrap();

    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        format!("python3 {}", script_path.display()),
        Value::Null,
    );
    let runtime = TestRuntimeView::new(InteractionResponse::Text {
        value: Some("alice".to_string()),
        display_value: None,
    });
    let input = PreToolHookInput {
        call: FinalToolCall {
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({"command": "pwd"}),
            ..Default::default()
        },
    };

    let output = adaptor
        .invoke_pre(&input, &HookInvokeMetadata::default(), &runtime)
        .await
        .unwrap();

    match output.primary {
        HookInvokePrimary::Pre(PreHookResult::Deny { reason }) => {
            assert_eq!(reason, "approved by alice");
        }
        other => panic!("unexpected output: {:?}", other),
    }

    let recorded_requests = runtime.interaction.recorded_requests();
    assert_eq!(recorded_requests.len(), 1);
    match &recorded_requests[0] {
        InteractionRequest::TextInput {
            prompt,
            source:
                Some(InteractionSource::Hooker {
                    hooker_name,
                    hook_point,
                }),
            is_secret: _, // Ignore is_secret in test
        } => {
            assert_eq!(prompt, "who approved this?");
            assert_eq!(hooker_name, "plugin_pre_ask");
            assert_eq!(hook_point, "test-agent.Tool.bash.pre");
        }
        other => panic!("unexpected interaction request: {:?}", other),
    }

    let _ = std::fs::remove_file(script_path);
}

#[test]
fn ask_user_requires_continuation_field() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    let error = adaptor
        .parse_plugin_command_response(json!({
            "action": "ask_user",
            "request": {
                "kind": "confirm",
                "prompt": "continue?",
                "source": null,
            }
        }))
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("response must contain field 'continuation'"));
}

#[test]
fn ask_user_request_parses_minimal_confirm() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    let parsed = adaptor
        .parse_plugin_command_response(json!({
            "action": "ask_user",
            "request": {
                "kind": "confirm",
                "prompt": "continue?"
            },
            "continuation": {"step": 1}
        }))
        .unwrap();

    match parsed {
        PluginCommandResponse::AskUser(AskUserDirective {
            request: PluginAskUserRequest::Confirm { prompt, .. },
            continuation,
        }) => {
            assert_eq!(prompt, "continue?");
            assert_eq!(continuation, json!({"step": 1}));
        }
        other => panic!("unexpected parsed response: {:?}", other),
    }
}

#[test]
fn final_responses_remain_backward_compatible() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    let legacy = adaptor
        .parse_plugin_command_response(json!({
            "result": "allow"
        }))
        .unwrap();
    let explicit = adaptor
        .parse_plugin_command_response(json!({
            "action": "final",
            "result": "allow"
        }))
        .unwrap();

    assert!(matches!(legacy, PluginCommandResponse::Final(_)));
    assert!(matches!(explicit, PluginCommandResponse::Final(_)));
}

/// Verify the plugin hooker child-process timeout + kill fallback chain:
/// spawn a `sleep 30` child (simulating a hang) with a 2s timeout, and assert:
/// 1. It returns ToolExecutionError::Timeout (instead of blocking forever);
/// 2. The elapsed time is near the timeout (~2s), proving the timeout fired
///    rather than the child exiting on its own;
/// 3. kill_on_drop kills the child after the future is dropped (no leftover
///    process check; guaranteed by kill_on_drop semantics).
#[tokio::test]
async fn run_plugin_command_times_out_and_kills_hung_child() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_timeout".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "sleep 30".to_string(),
        Value::Null,
    );
    let payload = json!({"stage": "pre"});

    let start = std::time::Instant::now();
    let result = adaptor
        .run_plugin_command_with_timeout(&payload, Duration::from_secs(2))
        .await;
    let elapsed = start.elapsed();

    // 1. Returns a Timeout error (not Ok, not other ExecutionFailed)
    match result {
        Err(ToolExecutionError::Timeout { timeout_ms }) => {
            assert_eq!(timeout_ms, 2000, "timeout_ms 应等于传入超时(2s=2000ms)");
        }
        other => panic!("expected Timeout, got: {:?}", other),
    }

    // 2. Elapsed time should be near the timeout (2s), allowing scheduling
    //    jitter, but far below the 30s of `sleep 30`.
    //    Upper bound 10s: exceeding it means the child was never killed by
    //    the timeout (still stuck).
    assert!(
        elapsed.as_secs() < 10,
        "超时应在 ~2s 触发，实际耗时 {:?}（疑似未被超时 kill）",
        elapsed
    );
    // Lower bound: must wait at least until the timeout threshold; an instant
    // return means the child never hung.
    assert!(
        elapsed.as_secs() >= 2,
        "应等到 2s 超时才返回，实际耗时 {:?}（疑似子进程未卡住）",
        elapsed
    );
}
