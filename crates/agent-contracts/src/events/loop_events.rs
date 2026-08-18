use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolResultEvent};

pub trait LoopEventSink: Send + Sync {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32);
    /// Progressive-snapshot contract: each call's `text` is a superset of
    /// the previous one, plus a final flush when the stream completes. The
    /// final flush may duplicate the last in-stream snapshot; sinks should
    /// replace (not append) with the most recent call's value.
    fn on_assistant_message(&self, agent_id: &AgentId, text: &str);
    /// Same progressive-snapshot contract as `on_assistant_message`, for
    /// reasoning/thinking content.
    fn on_assistant_reasoning(&self, _agent_id: &AgentId, _text: &str) {}
    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent);
    fn on_loop_end(&self, agent_id: &AgentId, summary: &LoopEndSummary);
}
