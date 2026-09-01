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
    /// Incremental delta variant of `on_assistant_message`. Sinks that can
    /// consume deltas should override this to avoid O(n²) full-text cloning
    /// on every stream chunk. The default implementation is a no-op so sinks
    /// that only need full snapshots (e.g. recording) are unaffected; the
    /// agent loop falls back to `on_assistant_message` when the sink does
    /// not report delta support via [`Self::supports_message_delta`].
    fn on_assistant_message_delta(&self, _agent_id: &AgentId, _delta: &str) {}
    /// Incremental delta variant of `on_assistant_reasoning`. See
    /// [`Self::on_assistant_message_delta`].
    fn on_assistant_reasoning_delta(&self, _agent_id: &AgentId, _delta: &str) {}
    /// Whether this sink prefers incremental deltas over full snapshots.
    /// When `true`, the agent loop calls `on_assistant_message_delta` /
    /// `on_assistant_reasoning_delta` instead of cloning the full
    /// accumulated text. Default `false` preserves the legacy full-snapshot
    /// behavior for sinks that have not opted in.
    fn supports_message_delta(&self) -> bool {
        false
    }
    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent);
    fn on_loop_end(&self, agent_id: &AgentId, summary: &LoopEndSummary);
}
