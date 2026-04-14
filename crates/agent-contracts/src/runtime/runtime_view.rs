use crate::events::tool_events::ToolEventSink;
use crate::hooker::registry::HookerRegistry;
use crate::interaction::InteractionHandle;
use crate::runtime::agent_context::AgentContext;
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
    /// Skill 注册表，工具执行时可通过此访问 skill 信息。None = 未配置
    fn skill_registry(&self) -> Option<&dyn SkillRegistry> {
        None
    }
}
