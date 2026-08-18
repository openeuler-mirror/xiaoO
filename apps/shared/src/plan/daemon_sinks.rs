use std::sync::Arc;

use agent_contracts::LoopEventSink;
use agent_types::common::ids::AgentId;
use agent_types::events::ToolResultEvent;

use super::{
    parse_spawn_subagent_agent_id_from_detail, parse_spawn_subagent_metadata_from_args,
    todo_snapshot_from_tool_args, SpawnSubagentMetadata, TodoSnapshotUpdate,
};

/// Receives computed [`TodoSnapshotUpdate`]s from the daemon so they can be
/// forwarded to remote clients (e.g. via an SSE channel). Mirrors
/// [`crate::session_diff::SessionDiffForwarder`].
pub trait PlanForwarder: Send + Sync {
    fn forward_plan(&self, snapshot: TodoSnapshotUpdate);
}

/// Receives computed [`SpawnSubagentMetadata`]s from the daemon so they can
/// be forwarded to remote clients. Mirrors
/// [`crate::session_diff::SessionDiffForwarder`].
pub trait SubagentMetaForwarder: Send + Sync {
    fn forward_subagent_meta(&self, metadata: SpawnSubagentMetadata);
}

/// Wraps an inner [`LoopEventSink`] and parses `todo_write` tool results in
/// `on_tool_result`, forwarding a structured [`TodoSnapshotUpdate`] via the
/// [`PlanForwarder`] before delegating to the inner sink. Mirrors
/// [`crate::session_diff::DiffComputingLoopSink`].
pub struct PlanComputingLoopSink {
    inner: Arc<dyn LoopEventSink>,
    forwarder: Arc<dyn PlanForwarder>,
}

impl PlanComputingLoopSink {
    pub fn new(inner: Arc<dyn LoopEventSink>, forwarder: Arc<dyn PlanForwarder>) -> Self {
        Self { inner, forwarder }
    }
}

impl LoopEventSink for PlanComputingLoopSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        self.inner.on_turn_start(agent_id, turn);
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        self.inner.on_assistant_message(agent_id, text);
    }

    fn on_assistant_reasoning(&self, agent_id: &AgentId, text: &str) {
        self.inner.on_assistant_reasoning(agent_id, text);
    }

    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent) {
        if event.tool_name == "todo_write" && !event.is_error {
            if let Some(snapshot) = todo_snapshot_from_tool_args(&event.args_preview) {
                self.forwarder.forward_plan(snapshot);
            }
        }
        self.inner.on_tool_result(agent_id, event);
    }

    fn on_loop_end(&self, agent_id: &AgentId, summary: &agent_types::events::LoopEndSummary) {
        self.inner.on_loop_end(agent_id, summary);
    }
}

/// Wraps an inner [`LoopEventSink`] and parses `spawn_subagent` tool results
/// in `on_tool_result`, forwarding structured [`SpawnSubagentMetadata`] via
/// the [`SubagentMetaForwarder`] before delegating to the inner sink. The
/// spawned child's `agent_id` is mined from the tool's output JSON carried
/// in `event.output_preview` (the SSE `ToolResult` event's `output_preview`
/// field); the parent agent id is taken from the `agent_id` argument the
/// `LoopEventSink::on_tool_result` callback receives.
pub struct SubagentMetaComputingLoopSink {
    inner: Arc<dyn LoopEventSink>,
    forwarder: Arc<dyn SubagentMetaForwarder>,
}

impl SubagentMetaComputingLoopSink {
    pub fn new(inner: Arc<dyn LoopEventSink>, forwarder: Arc<dyn SubagentMetaForwarder>) -> Self {
        Self { inner, forwarder }
    }
}

impl LoopEventSink for SubagentMetaComputingLoopSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        self.inner.on_turn_start(agent_id, turn);
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        self.inner.on_assistant_message(agent_id, text);
    }

    fn on_assistant_reasoning(&self, agent_id: &AgentId, text: &str) {
        self.inner.on_assistant_reasoning(agent_id, text);
    }

    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent) {
        if event.tool_name == "spawn_subagent" && !event.is_error {
            let parent_agent_id = if agent_id.0.is_empty() {
                None
            } else {
                Some(agent_id.0.clone())
            };
            let spawned_agent_id = parse_spawn_subagent_agent_id_from_detail(&event.output_preview);
            if let Some(metadata) = parse_spawn_subagent_metadata_from_args(
                &event.args_preview,
                parent_agent_id,
                spawned_agent_id.as_deref(),
            ) {
                self.forwarder.forward_subagent_meta(metadata);
            }
        }
        self.inner.on_tool_result(agent_id, event);
    }

    fn on_loop_end(&self, agent_id: &AgentId, summary: &agent_types::events::LoopEndSummary) {
        self.inner.on_loop_end(agent_id, summary);
    }
}
