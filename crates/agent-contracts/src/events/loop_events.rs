use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolResultEvent};

pub trait LoopEventSink: Send + Sync {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32);
    fn on_assistant_message(&self, agent_id: &AgentId, text: &str);
    fn on_assistant_reasoning(&self, _agent_id: &AgentId, _text: &str) {}
    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent);
    fn on_loop_end(&self, agent_id: &AgentId, summary: &LoopEndSummary);
}
