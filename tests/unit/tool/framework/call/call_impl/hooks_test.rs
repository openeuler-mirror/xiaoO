use std::any::Any;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_contracts::events::tool_events::ToolEventSink;
use agent_contracts::hook::registry::{Hooker, HookerRegistry};
use agent_contracts::interaction::handle::InteractionHandle;
use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::state::ToolStateStore;
use agent_contracts::tool::{ToolCall, ToolSpecView};
use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
use agent_types::common::{AgentMetadata, HookerId, WorkspaceRef};
use agent_types::events::ToolLifecycleEvent;
use agent_types::hook::{
    HookInvokeError, HookInvokeInput, HookInvokeOutput, HookInvokePrimary, HookPointId,
};
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::tool::{
    FinalToolCall, PostHookResult, PreHookResult, RawToolOutcome, ToolExecutionError,
    ToolExecutionResult, ToolLifecycleRecord,
};
use agent_types::ChatMessage;
use async_trait::async_trait;
use hook::escape_hook_point_segment;
use hook::framework::HookerRegistryImpl;

use super::super::tests::{test_execution_state, test_tool_call_impl};
use super::hooker_visible_call;

#[test]
fn hooker_visible_call_strips_accumulated_extra() {
    let final_call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        input: serde_json::json!({"command": "pwd"}),
        extra: Some(serde_json::json!([
            {"hooker_id": "plugin_a", "file": []}
        ])),
    };

    let visible = hooker_visible_call(&final_call);

    assert!(visible.extra.is_none());
    assert_eq!(visible.call_id, final_call.call_id);
    assert_eq!(visible.input, final_call.input);
    // the source keeps its extra for backend delivery
    assert!(final_call.extra.is_some());
}

#[test]
fn plugin_visible_call_serializes_without_extra_key() {
    let final_call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        input: serde_json::json!({"command": "pwd"}),
        extra: Some(serde_json::json!([
            {"hooker_id": "plugin_a", "file": []}
        ])),
    };

    let visible = hooker_visible_call(&final_call);
    let serialized = serde_json::to_value(&visible).unwrap();

    assert!(
        serialized.get("extra").is_none(),
        "plugin input must not carry an extra key"
    );
    assert_eq!(serialized["call_id"], "call-1");
    assert_eq!(serialized["tool_name"], "bash");

    // the accumulated call still serializes the extra channel for the backend
    let with_extra = serde_json::to_value(&final_call).unwrap();
    assert!(with_extra.get("extra").is_some());
}

// ---- minimal runtime for pre-hook sequence tests -----------------------

const TEST_AGENT_ID: &str = "test-agent";
const TEST_TOOL_NAME: &str = "test_tool";

fn pre_hook_point(tool_name: &str) -> HookPointId {
    HookPointId(format!(
        "{}.Tool.{}.pre",
        TEST_AGENT_ID,
        escape_hook_point_segment(tool_name)
    ))
}

/// What a scripted hooker does when invoked.
enum ScriptedOutput {
    Allow,
    Deny,
    InvokeError,
    NonPreVariant,
}

struct ScriptedHooker {
    id: HookerId,
    hook_point: HookPointId,
    output: ScriptedOutput,
    invocations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Hooker for ScriptedHooker {
    fn id(&self) -> &HookerId {
        &self.id
    }

    fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    async fn invoke(
        &self,
        _input: HookInvokeInput,
        _runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        self.invocations.lock().unwrap().push(self.id.0.clone());
        match self.output {
            ScriptedOutput::Allow => {
                Ok(HookInvokeOutput::Pre(PreHookResult::Allow { extra: None }))
            }
            ScriptedOutput::Deny => Ok(HookInvokeOutput::Pre(PreHookResult::Deny {
                reason: "scripted deny".to_string(),
            })),
            ScriptedOutput::InvokeError => {
                Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                    message: "hooker exploded".to_string(),
                }))
            }
            ScriptedOutput::NonPreVariant => Ok(HookInvokeOutput::new(HookInvokePrimary::Post(
                PostHookResult::Accept,
            ))),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct HookTestConversation;

impl ConversationView for HookTestConversation {
    fn recent_messages(&self, _limit: usize) -> Vec<ChatMessage> {
        Vec::new()
    }
    fn message_count(&self) -> usize {
        0
    }
}

struct HookTestAgentContext {
    conversation: HookTestConversation,
    workspace: WorkspaceRef,
    metadata: AgentMetadata,
}

impl HookTestAgentContext {
    fn new() -> Self {
        Self {
            conversation: HookTestConversation,
            workspace: WorkspaceRef {
                root: PathBuf::from("/tmp"),
            },
            metadata: AgentMetadata {
                agent_id: TEST_AGENT_ID.to_string(),
                model: "test-model".to_string(),
                session_id: None,
                task_label: None,
            },
        }
    }
}

impl AgentContext for HookTestAgentContext {
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

struct HookTestInteractionHandle;

#[async_trait]
impl InteractionHandle for HookTestInteractionHandle {
    async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
        InteractionResponse::Confirmed { allowed: false }
    }
}

/// Panics on `begin`: deny-path tests must never reach lifecycle setup, and
/// reaching it fails the test instead of silently executing the tool.
struct HookTestStateStore;

impl ToolStateStore for HookTestStateStore {
    fn begin(&self, _call: &FinalToolCall, _spec: &dyn ToolSpecView) -> ToolLifecycleRecord {
        panic!("state store must not be reached in pre-hook deny tests")
    }
    fn update(&self, _record: &ToolLifecycleRecord) {}
    fn finish(&self, _record: &ToolLifecycleRecord, _result: &ToolExecutionResult) {}
    fn fail(&self, _record: &ToolLifecycleRecord, _error: &ToolExecutionError) {}
}

struct HookTestToolEventSink;

impl ToolEventSink for HookTestToolEventSink {
    fn emit(&self, _event: ToolLifecycleEvent) {}
}

struct HookTestTraceRecorder;

#[async_trait]
impl TraceRecorder for HookTestTraceRecorder {
    async fn begin_span(
        &self,
        _kind: TraceSpanKind,
        _name: Cow<'static, str>,
        _fields: serde_json::Value,
    ) -> TraceSpanHandle {
        TraceSpanHandle::new("trace-test", "span-test", None)
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
}

struct HookTestRuntime {
    agent_context: HookTestAgentContext,
    hookers: Arc<dyn HookerRegistry>,
    interaction: HookTestInteractionHandle,
    state_store: HookTestStateStore,
    tool_events: HookTestToolEventSink,
    trace_recorder: HookTestTraceRecorder,
}

impl RuntimeView for HookTestRuntime {
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
        self.hookers.as_ref()
    }
}

fn scripted_hooker(
    hook_point: HookPointId,
    id: &str,
    output: ScriptedOutput,
    invocations: &Arc<Mutex<Vec<String>>>,
) -> ScriptedHooker {
    ScriptedHooker {
        id: HookerId(id.to_string()),
        hook_point,
        output,
        invocations: Arc::clone(invocations),
    }
}

/// Build a runtime around the production registry so both exact and wildcard
/// hook point patterns go through the real segment matcher.
fn hook_test_runtime(hookers: Vec<ScriptedHooker>) -> HookTestRuntime {
    let enabled: HashSet<HookerId> = hookers.iter().map(|hooker| hooker.id().clone()).collect();
    let hookers: HashMap<HookerId, Box<dyn Hooker>> = hookers
        .into_iter()
        .map(|hooker| (hooker.id().clone(), Box::new(hooker) as Box<dyn Hooker>))
        .collect();
    HookTestRuntime {
        agent_context: HookTestAgentContext::new(),
        hookers: Arc::new(HookerRegistryImpl::new(hookers, enabled, HashMap::new())),
        interaction: HookTestInteractionHandle,
        state_store: HookTestStateStore,
        tool_events: HookTestToolEventSink,
        trace_recorder: HookTestTraceRecorder,
    }
}

// ---- fail-closed pre-hook tests ----------------------------------------

#[tokio::test]
async fn pre_hooker_invoke_error_denies_and_stops_sequence() {
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let runtime = hook_test_runtime(vec![
        scripted_hooker(
            pre_hook_point(TEST_TOOL_NAME),
            "a",
            ScriptedOutput::Allow,
            &invocations,
        ),
        scripted_hooker(
            pre_hook_point(TEST_TOOL_NAME),
            "b",
            ScriptedOutput::InvokeError,
            &invocations,
        ),
        scripted_hooker(
            pre_hook_point(TEST_TOOL_NAME),
            "c",
            ScriptedOutput::Allow,
            &invocations,
        ),
    ]);
    let call_impl = test_tool_call_impl(TEST_TOOL_NAME);
    let mut state = test_execution_state(TEST_TOOL_NAME);

    let results = call_impl
        .run_pre_hook_sequence(&mut state, &runtime)
        .await
        .expect("fail-closed deny is not a sequence error");

    assert_eq!(
        *invocations.lock().unwrap(),
        vec!["a".to_string(), "b".to_string()],
        "sequence must stop at the failing hooker, later hookers must not run"
    );
    assert_eq!(results.len(), 2);
    assert!(matches!(&results[0], PreHookResult::Allow { extra: None }));
    match &results[1] {
        PreHookResult::Deny { reason } => {
            assert!(
                reason.contains("'b'") && reason.contains("fail-closed"),
                "reason must name the failed hooker and be fail-closed: {reason}"
            );
            assert!(
                reason.contains("hooker exploded"),
                "reason must carry the invoke error: {reason}"
            );
        }
        other => panic!("expected deny, got {:?}", other),
    }
}

#[tokio::test]
async fn pre_hooker_non_pre_output_denies_and_stops_sequence() {
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let runtime = hook_test_runtime(vec![
        scripted_hooker(
            pre_hook_point(TEST_TOOL_NAME),
            "a",
            ScriptedOutput::NonPreVariant,
            &invocations,
        ),
        scripted_hooker(
            pre_hook_point(TEST_TOOL_NAME),
            "b",
            ScriptedOutput::Allow,
            &invocations,
        ),
    ]);
    let call_impl = test_tool_call_impl(TEST_TOOL_NAME);
    let mut state = test_execution_state(TEST_TOOL_NAME);

    let results = call_impl
        .run_pre_hook_sequence(&mut state, &runtime)
        .await
        .expect("fail-closed deny is not a sequence error");

    assert_eq!(
        *invocations.lock().unwrap(),
        vec!["a".to_string()],
        "sequence must stop at the offending hooker"
    );
    assert_eq!(results.len(), 1);
    match &results[0] {
        PreHookResult::Deny { reason } => {
            assert!(
                reason.contains("'a'") && reason.contains("non-pre output"),
                "reason must name the offending hooker and the variant mismatch: {reason}"
            );
        }
        other => panic!("expected deny, got {:?}", other),
    }
}

#[test]
fn build_tool_hook_point_escapes_dotted_tool_name() {
    // the hook point of a dotted tool name must stay at exactly 4 segments
    // so category resolution succeeds and 4-segment patterns can match it
    let call_impl = test_tool_call_impl("plugin.tool");
    let runtime = hook_test_runtime(Vec::new());

    let dotted = call_impl.build_tool_hook_point(&runtime, "plugin.tool", "pre");
    assert_eq!(dotted.0, "test-agent.Tool.plugin%2Etool.pre");
}

#[tokio::test]
async fn wildcard_pre_hooker_denies_dotted_tool_name() {
    // `*.Tool.*.pre` is the only form plugin hookers can register (their
    // hook points are validated to 4 segments at build time); after
    // escaping, a dotted tool name must still match it, so a denying
    // wildcard (e.g. an audit plugin) blocks the call instead of the check
    // being silently skipped. The panicking state store and executor prove
    // the call never proceeds past the deny.
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let runtime = hook_test_runtime(vec![scripted_hooker(
        HookPointId("*.Tool.*.pre".to_string()),
        "guard",
        ScriptedOutput::Deny,
        &invocations,
    )]);
    let call_impl = test_tool_call_impl("plugin.tool");

    let result = call_impl.execute(&runtime).await.expect("execution result");

    match result {
        ToolExecutionResult::Denied {
            pre_hook_results, ..
        } => {
            assert!(
                pre_hook_results
                    .iter()
                    .any(|result| matches!(result, PreHookResult::Deny { .. })),
                "the wildcard hooker's deny must be recorded: {pre_hook_results:?}"
            );
        }
        other => panic!("expected denied result, got {:?}", other),
    }
    assert_eq!(
        *invocations.lock().unwrap(),
        vec!["guard".to_string()],
        "the wildcard pre-hooker must run for a dotted tool name"
    );
}

#[tokio::test]
async fn hook_phases_skip_dotted_tool_name_without_hookers() {
    // dotted tool name, no hookers attached: every phase is skipped so
    // hook-free deployments never fail on category resolution
    let call_impl = test_tool_call_impl("plugin.tool");
    let runtime = hook_test_runtime(Vec::new());
    let mut state = test_execution_state("plugin.tool");

    let pre = call_impl
        .run_pre_hook_sequence(&mut state, &runtime)
        .await
        .expect("no hookers means no phase error, even for a dotted tool name");
    assert!(pre.is_empty());

    state.raw_outcome = Some(RawToolOutcome::Success {
        output: "ok".to_string(),
    });
    let post = call_impl
        .run_post_hook_sequence(&mut state, &runtime)
        .await
        .expect("no hookers means no phase error, even for a dotted tool name");
    assert!(post.is_empty());

    let execution_error = ToolExecutionError::ExecutionFailed {
        message: "boom".to_string(),
    };
    let error = call_impl
        .run_error_hook_sequence(&state, &execution_error, &runtime)
        .await
        .expect("no hookers means no phase error, even for a dotted tool name");
    assert!(error.is_empty());
}
