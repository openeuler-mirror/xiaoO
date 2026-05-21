use std::sync::Arc;

use agent_contracts::events::LoopEventSink;
use agent_contracts::interaction::InteractionHandle;
use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::AgentId;
use agent_types::ReasoningEffort;
use async_trait::async_trait;

#[async_trait]
pub trait PendingUserMessageSource: Send + Sync {
    async fn drain_pending_user_messages(&self) -> Vec<String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopStopRule {
    AfterSuccessfulTool { tool_name: String },
}

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
    pub reasoning_effort: ReasoningEffort,
    pub stop_rules: Vec<LoopStopRule>,
    // User messages from gateway.
    pub pending_user_messages: Option<Arc<dyn PendingUserMessageSource>>,
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
            reasoning_effort: ReasoningEffort::Off,
            stop_rules: Vec::new(),
            pending_user_messages: None,
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

    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = effort;
        self
    }

    pub fn with_stop_rules(mut self, rules: impl IntoIterator<Item = LoopStopRule>) -> Self {
        self.stop_rules = rules.into_iter().collect();
        self
    }

    pub fn with_pending_user_messages(mut self, source: Arc<dyn PendingUserMessageSource>) -> Self {
        self.pending_user_messages = Some(source);
        self
    }

    pub fn resume_without_user_message(mut self) -> Self {
        self.append_user_message = false;
        self
    }
}
