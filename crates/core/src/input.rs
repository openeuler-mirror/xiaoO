use std::sync::Arc;

use agent_contracts::events::LoopEventSink;
use agent_contracts::interaction::InteractionHandle;
use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::AgentId;

pub struct AgentLoopInput {
    pub user_message: String,
    pub append_user_message: bool,
    pub event_sink: Option<Arc<dyn LoopEventSink>>,
    pub interaction: Option<Arc<dyn InteractionHandle>>,
    pub agent_id: Option<AgentId>,
    /// Gateway 预解析的可见工具集（`Arc` 因为 `PromptBuildInput` 需要 owned）。
    pub visible_tools: Vec<Arc<dyn ToolSpecView>>,
    /// `None` = tool execution 跳过。
    pub runtime_view: Option<Arc<dyn RuntimeView>>,
}

impl AgentLoopInput {
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
            append_user_message: true,
            event_sink: None,
            interaction: None,
            agent_id: None,
            visible_tools: Vec::new(),
            runtime_view: None,
        }
    }

    pub fn with_event_sink(mut self, sink: Arc<dyn LoopEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    pub fn with_interaction(mut self, interaction: Arc<dyn InteractionHandle>) -> Self {
        self.interaction = Some(interaction);
        self
    }

    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn with_visible_tools(mut self, tools: Vec<Arc<dyn ToolSpecView>>) -> Self {
        self.visible_tools = tools;
        self
    }

    pub fn with_runtime_view(mut self, view: Arc<dyn RuntimeView>) -> Self {
        self.runtime_view = Some(view);
        self
    }

    pub fn resume_without_user_message(mut self) -> Self {
        self.append_user_message = false;
        self
    }
}
