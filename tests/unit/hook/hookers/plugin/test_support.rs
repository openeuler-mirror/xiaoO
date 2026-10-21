//! Test-only helpers shared across the plugin adaptor test modules.

use std::borrow::Cow;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;

use agent_contracts::events::tool_events::ToolEventSink;
use agent_contracts::hook::registry::{Hooker, HookerRegistry};
use agent_contracts::interaction::handle::InteractionHandle;
use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::spec::ToolSpecView;
use agent_contracts::tool::state::ToolStateStore;
use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
use agent_types::common::HookerId;
use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::events::ToolLifecycleEvent;
use agent_types::hook::HookPointId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::tool::execution_types::{ToolExecutionError, ToolExecutionResult};
use agent_types::tool::{FinalToolCall, ToolLifecycleRecord};
use agent_types::ChatMessage;

/// Run a future on a freshly built current-thread tokio runtime with all
/// drivers enabled (IO + time + process). `tokio::process` /
/// `tokio::time::timeout` (used by `run_plugin_subprocess`) need a real
/// runtime; a noop-waker poll loop cannot drive them.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for test")
        .block_on(future)
}

// ---- minimal test runtime view --------------------------------------

pub(crate) struct TestConversation;

impl ConversationView for TestConversation {
    fn recent_messages(&self, _limit: usize) -> Vec<ChatMessage> {
        Vec::new()
    }
    fn message_count(&self) -> usize {
        0
    }
}

pub(crate) struct TestAgentContext {
    conversation: TestConversation,
    workspace: WorkspaceRef,
    metadata: AgentMetadata,
}

impl TestAgentContext {
    pub(crate) fn new() -> Self {
        Self::with_workspace_and_session(PathBuf::from("/tmp"), Some("session-1".to_string()))
    }

    pub(crate) fn with_workspace_and_session(
        workspace: PathBuf,
        session_id: Option<String>,
    ) -> Self {
        Self {
            conversation: TestConversation,
            workspace: WorkspaceRef { root: workspace },
            metadata: AgentMetadata {
                agent_id: "test-agent".to_string(),
                model: "test-model".to_string(),
                session_id,
                task_label: None,
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

/// Interaction handle that replays a configured response and records every
/// request it was asked.
pub(crate) struct TestInteractionHandle {
    response: InteractionResponse,
    requests: Mutex<Vec<InteractionRequest>>,
}

impl TestInteractionHandle {
    pub(crate) fn new(response: InteractionResponse) -> Self {
        Self {
            response,
            requests: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn recorded_requests(&self) -> Vec<InteractionRequest> {
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

pub(crate) struct TestHookerRegistry;

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

pub(crate) struct TestToolStateStore;

impl ToolStateStore for TestToolStateStore {
    fn begin(&self, _call: &FinalToolCall, _spec: &dyn ToolSpecView) -> ToolLifecycleRecord {
        panic!("unused in plugin adaptor test")
    }
    fn update(&self, _record: &ToolLifecycleRecord) {}
    fn finish(&self, _record: &ToolLifecycleRecord, _result: &ToolExecutionResult) {}
    fn fail(&self, _record: &ToolLifecycleRecord, _error: &ToolExecutionError) {}
}

pub(crate) struct TestToolEventSink;

impl ToolEventSink for TestToolEventSink {
    fn emit(&self, _event: ToolLifecycleEvent) {}
}

pub(crate) struct TestTraceRecorder;

#[async_trait]
impl TraceRecorder for TestTraceRecorder {
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
    async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: serde_json::Value) {}
}

/// Minimal [`RuntimeView`] for plugin adaptor tests: empty conversation,
/// no hookers, no-op state store / event sink, and an interaction handle
/// whose behaviour is chosen per test via the constructors.
pub(crate) struct TestRuntimeView {
    pub(crate) interaction: TestInteractionHandle,
    state_store: TestToolStateStore,
    tool_events: TestToolEventSink,
    trace_recorder: TestTraceRecorder,
    agent_context: TestAgentContext,
    hookers: TestHookerRegistry,
}

impl TestRuntimeView {
    /// View whose interaction handle denies confirmations.
    pub(crate) fn new() -> Self {
        Self::with_interaction_response(InteractionResponse::Confirmed { allowed: false })
    }

    /// View whose interaction handle replays `response`.
    pub(crate) fn with_interaction_response(response: InteractionResponse) -> Self {
        Self::assemble(
            TestAgentContext::new(),
            TestInteractionHandle::new(response),
        )
    }

    /// View with a custom agent context (workspace / session overrides).
    pub(crate) fn with_agent_context(agent_context: TestAgentContext) -> Self {
        Self::assemble(
            agent_context,
            TestInteractionHandle::new(InteractionResponse::Confirmed { allowed: false }),
        )
    }

    fn assemble(agent_context: TestAgentContext, interaction: TestInteractionHandle) -> Self {
        Self {
            interaction,
            state_store: TestToolStateStore,
            tool_events: TestToolEventSink,
            trace_recorder: TestTraceRecorder,
            agent_context,
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

/// Write `json` to a temp file and return a `sh -c` command string that
/// drains stdin to `/dev/null` then prints the file. Draining stdin avoids
/// a broken-pipe race with `run_plugin_command`, which always writes a
/// payload to the child's stdin even though these canned-response tests
/// ignore it. A monotonic counter guarantees a unique filename even when
/// two tests allocate one in the same nanosecond (which `SystemTime` could
/// not, causing flaky cross-test collisions). `prefix` identifies the
/// consuming test module in the filename.
pub(crate) fn cat_command_for(prefix: &str, json: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "{}_{}_{}.json",
        prefix,
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::write(&path, json).unwrap();
    format!("cat > /dev/null; cat {}", path.display())
}
