use std::path::PathBuf;

use agent_contracts::{
    AgentContext, ConversationView, HookerRegistry, InteractionHandle, RuntimeView, SkillRegistry,
    ToolEventSink, ToolStateStore, TraceRecorder,
};
use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::context::prompt::SkillSummary;
use agent_types::events::ToolLifecycleEvent;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::ChatMessage;
use async_trait::async_trait;

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
