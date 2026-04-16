use crate::events::tool_events::ToolEventSink;
use crate::hooker::registry::HookerRegistry;
use crate::interaction::InteractionHandle;
use crate::runtime::agent_context::AgentContext;
use crate::runtime::channel_file_sender::ChannelFileSender;
use crate::skill::registry::SkillRegistry;
use crate::tool::state::ToolStateStore;
use crate::trace::TraceRecorder;

pub trait RuntimeView: Send + Sync {
    fn state_store(&self) -> &dyn ToolStateStore;
    fn tool_events(&self) -> &dyn ToolEventSink;
    fn trace_recorder(&self) -> &dyn TraceRecorder;
    fn agent_context(&self) -> &dyn AgentContext;
    fn interaction(&self) -> &dyn InteractionHandle;
    fn hookers(&self) -> &dyn HookerRegistry;
    /// Skill registry for tool-time skill lookups. None = not configured.
    fn skill_registry(&self) -> Option<&dyn SkillRegistry> {
        None
    }
    /// Channel file sender for sending files to the user. None = not a channel session.
    fn channel_file_sender(&self) -> Option<&dyn ChannelFileSender> {
        None
    }
}
