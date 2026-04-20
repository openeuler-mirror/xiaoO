use std::borrow::Cow;
use std::path::PathBuf;

use agent_contracts::{
    AgentContext, ConversationView, Hooker, HookerRegistry, InteractionHandle, RuntimeView,
    SkillRegistry, ToolEventSink, ToolSpecView, ToolStateStore, TraceOutcome, TraceRecorder,
    TraceSpanHandle, TraceSpanKind,
};
use agent_types::common::{AgentMetadata, HookerId, WorkspaceRef};
use agent_types::context::prompt::SkillSummary;
use agent_types::events::ToolLifecycleEvent;
use agent_types::hook::HookPointId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::tool::{
    FinalToolCall, ToolExecutionError, ToolExecutionResult, ToolLifecycleRecord,
    ToolLifecycleStatus,
};
use agent_types::ChatMessage;
use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopToolEventSink;

impl NoopToolEventSink {
    pub fn new() -> Self {
        Self
    }
}

impl ToolEventSink for NoopToolEventSink {
    fn emit(&self, _event: ToolLifecycleEvent) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopInteractionHandle;

impl NoopInteractionHandle {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl InteractionHandle for NoopInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        match request {
            InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
            InteractionRequest::TextInput { .. } => InteractionResponse::Text { value: None },
            InteractionRequest::Choice { .. } => InteractionResponse::Choice { value: None },
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EmptySkillRegistry;

impl EmptySkillRegistry {
    pub fn new() -> Self {
        Self
    }
}

impl SkillRegistry for EmptySkillRegistry {
    fn list_skills(&self) -> Vec<SkillSummary> {
        Vec::new()
    }

    fn get_skill(&self, _skill_id: &str) -> Option<&dyn agent_contracts::skill::SkillSpec> {
        None
    }
}

pub struct OwnedConversationView {
    messages: Vec<ChatMessage>,
}

impl OwnedConversationView {
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self { messages }
    }
}

impl ConversationView for OwnedConversationView {
    fn recent_messages(&self, limit: usize) -> &[ChatMessage] {
        let len = self.messages.len();
        let start = len.saturating_sub(limit);
        &self.messages[start..]
    }

    fn message_count(&self) -> usize {
        self.messages.len()
    }
}

pub struct BasicAgentContext {
    conversation: OwnedConversationView,
    workspace: WorkspaceRef,
    metadata: AgentMetadata,
}

impl BasicAgentContext {
    pub fn new(
        messages: Vec<ChatMessage>,
        workspace_root: PathBuf,
        metadata: AgentMetadata,
    ) -> Self {
        Self {
            conversation: OwnedConversationView::new(messages),
            workspace: WorkspaceRef {
                root: workspace_root,
            },
            metadata,
        }
    }
}

impl AgentContext for BasicAgentContext {
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

pub struct BasicRuntimeView {
    state_store: Box<dyn ToolStateStore>,
    tool_events: Box<dyn ToolEventSink>,
    trace_recorder: Box<dyn TraceRecorder>,
    agent_context: Box<dyn AgentContext>,
    interaction: Box<dyn InteractionHandle>,
    hookers: Box<dyn HookerRegistry>,
}

impl BasicRuntimeView {
    pub fn new(
        state_store: Box<dyn ToolStateStore>,
        tool_events: Box<dyn ToolEventSink>,
        trace_recorder: Box<dyn TraceRecorder>,
        agent_context: Box<dyn AgentContext>,
        interaction: Box<dyn InteractionHandle>,
        hookers: Box<dyn HookerRegistry>,
    ) -> Self {
        Self {
            state_store,
            tool_events,
            trace_recorder,
            agent_context,
            interaction,
            hookers,
        }
    }
}

impl RuntimeView for BasicRuntimeView {
    fn state_store(&self) -> &dyn ToolStateStore {
        self.state_store.as_ref()
    }

    fn tool_events(&self) -> &dyn ToolEventSink {
        self.tool_events.as_ref()
    }

    fn trace_recorder(&self) -> &dyn TraceRecorder {
        self.trace_recorder.as_ref()
    }

    fn agent_context(&self) -> &dyn AgentContext {
        self.agent_context.as_ref()
    }

    fn interaction(&self) -> &dyn InteractionHandle {
        self.interaction.as_ref()
    }

    fn hookers(&self) -> &dyn HookerRegistry {
        self.hookers.as_ref()
    }
}

struct NoopToolStateStore;

impl ToolStateStore for NoopToolStateStore {
    fn begin(&self, call: &FinalToolCall, _spec: &dyn ToolSpecView) -> ToolLifecycleRecord {
        ToolLifecycleRecord {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            status: ToolLifecycleStatus::Pending,
            started_at_ms: 0,
            finished_at_ms: None,
        }
    }

    fn update(&self, _record: &ToolLifecycleRecord) {}

    fn finish(&self, _record: &ToolLifecycleRecord, _result: &ToolExecutionResult) {}

    fn fail(&self, _record: &ToolLifecycleRecord, _error: &ToolExecutionError) {}
}

struct NoopTraceRecorder;

#[async_trait]
impl TraceRecorder for NoopTraceRecorder {
    async fn begin_span(
        &self,
        _kind: TraceSpanKind,
        _name: Cow<'static, str>,
        _fields: Value,
    ) -> TraceSpanHandle {
        TraceSpanHandle::new("", "", None)
    }

    async fn update_span(&self, _span: &TraceSpanHandle, _fields: Value) {}

    async fn end_span(&self, _span: TraceSpanHandle, _outcome: TraceOutcome, _fields: Value) {}

    async fn finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {}

    async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {}
}

struct NoopHookerRegistry;

impl HookerRegistry for NoopHookerRegistry {
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

/// A minimal no-op RuntimeView for use in contexts that require a RuntimeView
/// but do not need any of its capabilities (e.g. session lifecycle hooks).
pub struct NoopRuntimeView {
    state_store: NoopToolStateStore,
    tool_events: NoopToolEventSink,
    trace_recorder: NoopTraceRecorder,
    agent_context: BasicAgentContext,
    interaction: NoopInteractionHandle,
    hookers: NoopHookerRegistry,
}

impl NoopRuntimeView {
    pub fn new() -> Self {
        Self {
            state_store: NoopToolStateStore,
            tool_events: NoopToolEventSink,
            trace_recorder: NoopTraceRecorder,
            agent_context: BasicAgentContext::new(
                Vec::new(),
                PathBuf::from("."),
                AgentMetadata {
                    agent_id: String::new(),
                    model: String::new(),
                    session_id: None,
                },
            ),
            interaction: NoopInteractionHandle,
            hookers: NoopHookerRegistry,
        }
    }
}

impl Default for NoopRuntimeView {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeView for NoopRuntimeView {
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
