use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::response::sse;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use xiaoo_api::chat::AgentId;
use xiaoo_api::events::{
    LoopEndSummary, LoopEventSink, ToolEventSink, ToolLifecycleEvent, ToolResultEvent,
};
pub use xiaoo_shared::daemon_protocol::sse::{RuntimeSseEvent as SseStreamEvent, ToolCallStatus};
use xiaoo_shared::plan::{
    PlanForwarder, SpawnSubagentMetadata, SubagentMetaForwarder, TodoSnapshotUpdate,
};
use xiaoo_shared::session_diff::{FileChangeDelta, SessionDiffForwarder};

pub struct SseLoopEventSink {
    tx: mpsc::UnboundedSender<SseStreamEvent>,
    last_snapshot_len: Mutex<BTreeMap<String, usize>>,
    last_thinking_snapshot_len: Mutex<BTreeMap<String, usize>>,
    loop_summary: Mutex<Option<LoopEndSummary>>,
}

impl SseLoopEventSink {
    pub fn new(tx: mpsc::UnboundedSender<SseStreamEvent>) -> Self {
        Self {
            tx,
            last_snapshot_len: Mutex::new(BTreeMap::new()),
            last_thinking_snapshot_len: Mutex::new(BTreeMap::new()),
            loop_summary: Mutex::new(None),
        }
    }

    pub fn take_loop_summary(&self) -> Option<LoopEndSummary> {
        self.loop_summary
            .lock()
            .expect("sse sink loop_summary mutex should not be poisoned")
            .take()
    }
}

impl LoopEventSink for SseLoopEventSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        if let Ok(mut len) = self.last_snapshot_len.lock() {
            len.insert(agent_id.0.clone(), 0);
        }
        if let Ok(mut len) = self.last_thinking_snapshot_len.lock() {
            len.insert(agent_id.0.clone(), 0);
        }
        let _ = self.tx.send(SseStreamEvent::TurnStart {
            agent_id: agent_id.0.clone(),
            turn,
        });
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        let delta = {
            let mut last_len = self
                .last_snapshot_len
                .lock()
                .expect("sse sink last_snapshot_len mutex should not be poisoned");
            let prev = *last_len.get(&agent_id.0).unwrap_or(&0);
            last_len.insert(agent_id.0.clone(), text.len());
            if prev < text.len() {
                text[prev..].to_string()
            } else {
                return;
            }
        };
        let _ = self.tx.send(SseStreamEvent::TextDelta {
            agent_id: agent_id.0.clone(),
            delta,
            snapshot: text.to_string(),
        });
    }

    fn on_assistant_reasoning(&self, agent_id: &AgentId, text: &str) {
        let delta = {
            let mut last_len = self
                .last_thinking_snapshot_len
                .lock()
                .expect("sse sink last_thinking_snapshot_len mutex should not be poisoned");
            let prev = *last_len.get(&agent_id.0).unwrap_or(&0);
            last_len.insert(agent_id.0.clone(), text.len());
            if prev < text.len() {
                text[prev..].to_string()
            } else {
                return;
            }
        };
        let _ = self.tx.send(SseStreamEvent::ThinkingDelta {
            agent_id: agent_id.0.clone(),
            delta,
            snapshot: text.to_string(),
        });
    }

    /// Forwards a `ToolResultEvent` as an SSE `ToolResult` event. The
    /// tool framework guarantees this is called AFTER the corresponding
    /// `ToolLifecycleEvent::Running` (which `SseToolEventSink::emit`
    /// forwards as a `ToolCall(running)` SSE event), so the TUI's
    /// tool-card state machine sees `Running` -> `Completed`/`Failed`
    /// in order.
    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent) {
        let _ = self.tx.send(SseStreamEvent::ToolResult {
            agent_id: agent_id.0.clone(),
            call_id: event.call_id.clone(),
            tool_name: event.tool_name.clone(),
            output_preview: event.output_preview.clone(),
            is_error: event.is_error,
            args_preview: event.args_preview.clone(),
        });
    }

    fn on_loop_end(&self, agent_id: &AgentId, summary: &LoopEndSummary) {
        if let Ok(mut stored) = self.loop_summary.lock() {
            *stored = Some(summary.clone());
        }
        // Emit a per-agent `LoopEnd` marker so the TUI clears
        // `is_running` on the matching lane. Summary fields forward the
        // per-agent `LoopEndSummary`.
        let _ = self.tx.send(SseStreamEvent::LoopEnd {
            agent_id: agent_id.0.clone(),
            turn_count: summary.turn_count,
            total_tokens: summary.total_tokens,
            stop_reason: summary.stop_reason.clone(),
        });
    }
}

pub fn sse_stream_from_receiver(
    rx: mpsc::UnboundedReceiver<SseStreamEvent>,
) -> impl futures_util::Stream<Item = Result<sse::Event, Infallible>> {
    UnboundedReceiverStream::new(rx).map(|event| {
        let name = event.event_name();
        let data =
            serde_json::to_string(&event).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
        Ok(sse::Event::default().event(name).data(data))
    })
}

/// Bridges computed [`FileChangeDelta`]s into the
/// SSE stream by emitting [`SseStreamEvent::ToolFileChange`] events on the
/// same `mpsc` channel that carries the rest of the SSE traffic. Used by the
/// daemon-side `DiffComputingLoopSink` so the remote TUI can render session
/// diff without re-reading the daemon's filesystem.
#[derive(Clone)]
pub struct SseDeltaForwarder {
    tx: mpsc::UnboundedSender<SseStreamEvent>,
}

impl SseDeltaForwarder {
    pub fn new(tx: mpsc::UnboundedSender<SseStreamEvent>) -> Self {
        Self { tx }
    }
}

impl SessionDiffForwarder for SseDeltaForwarder {
    fn forward_delta(&self, call_id: &str, delta: FileChangeDelta) {
        let _ = self.tx.send(SseStreamEvent::ToolFileChange {
            call_id: call_id.to_string(),
            file_path: delta.file_path,
            additions: delta.additions,
            deletions: delta.deletions,
        });
    }
}

/// Bridges computed [`TodoSnapshotUpdate`]s into the SSE
/// stream by emitting [`SseStreamEvent::PlanUpdate`] events. Mirrors
/// [`SseDeltaForwarder`].
#[derive(Clone)]
pub struct SsePlanForwarder {
    tx: mpsc::UnboundedSender<SseStreamEvent>,
}

impl SsePlanForwarder {
    pub fn new(tx: mpsc::UnboundedSender<SseStreamEvent>) -> Self {
        Self { tx }
    }
}

impl PlanForwarder for SsePlanForwarder {
    fn forward_plan(&self, snapshot: TodoSnapshotUpdate) {
        let _ = self.tx.send(SseStreamEvent::PlanUpdate {
            title: snapshot.title,
            items: snapshot.items,
        });
    }
}

/// Bridges computed [`SpawnSubagentMetadata`]s into the
/// SSE stream by emitting [`SseStreamEvent::SubagentSpawn`] events. Mirrors
/// [`SseDeltaForwarder`].
#[derive(Clone)]
pub struct SseSubagentMetaForwarder {
    tx: mpsc::UnboundedSender<SseStreamEvent>,
}

impl SseSubagentMetaForwarder {
    pub fn new(tx: mpsc::UnboundedSender<SseStreamEvent>) -> Self {
        Self { tx }
    }
}

impl SubagentMetaForwarder for SseSubagentMetaForwarder {
    fn forward_subagent_meta(&self, metadata: SpawnSubagentMetadata) {
        let _ = self.tx.send(SseStreamEvent::SubagentSpawn {
            agent_id: metadata.agent_id,
            parent_agent_id: metadata.parent_agent_id,
            title: metadata.title,
            description: metadata.description,
            task_goal: metadata.task_goal,
        });
    }
}

/// Forwards tool-lifecycle events to the remote TUI as
/// [`SseStreamEvent::ToolCall`] SSE events. Wraps an inner
/// [`ToolEventSink`] so its side effects (e.g. diff baseline capture)
/// keep firing alongside the SSE forwarding.
///
/// Only `Running`/`Pending` lifecycle events are forwarded as SSE
/// `ToolCall` events; terminal states (`Completed`/`Failed`/`Denied`)
/// are conveyed by the subsequent `ToolResult` SSE event emitted by
/// `SseLoopEventSink::on_tool_result`, which also carries
/// `output_preview`. This avoids duplicate terminal updates and
/// ordering races between the two sinks.
pub struct SseToolEventSink {
    tx: mpsc::UnboundedSender<SseStreamEvent>,
    inner: Option<Arc<dyn ToolEventSink>>,
}

impl SseToolEventSink {
    /// Wrap an existing [`ToolEventSink`] so its side effects (e.g. diff
    /// baseline capture) keep firing alongside the new SSE forwarding.
    pub fn with_inner(
        tx: mpsc::UnboundedSender<SseStreamEvent>,
        inner: Arc<dyn ToolEventSink>,
    ) -> Self {
        Self {
            tx,
            inner: Some(inner),
        }
    }
}

/// Recursively unwrap `AgentScoped` layers and extract wire-format
/// fields. The innermost `agent_id` wins; `fallback_agent_id` is used
/// for un-scoped root events.
fn flatten_tool_lifecycle(
    event: ToolLifecycleEvent,
    fallback_agent_id: AgentId,
) -> (AgentId, String, String, String, ToolCallStatus, String) {
    match event {
        ToolLifecycleEvent::AgentScoped { agent_id, event } => {
            flatten_tool_lifecycle(*event, agent_id)
        }
        ToolLifecycleEvent::Pending {
            call_id,
            tool_name,
            args_preview,
        }
        | ToolLifecycleEvent::Running {
            call_id,
            tool_name,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Running,
            String::new(),
        ),
        ToolLifecycleEvent::Completed {
            call_id,
            tool_name,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Completed,
            String::new(),
        ),
        ToolLifecycleEvent::Failed {
            call_id,
            tool_name,
            error,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Failed,
            error,
        ),
        ToolLifecycleEvent::Denied {
            call_id,
            tool_name,
            reason,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Denied,
            reason,
        ),
    }
}

/// Fallback `agent_id` for un-scoped root tool-lifecycle events. Using
/// `"root"` instead of an empty string ensures the TUI routes the event
/// to the root message list rather than silently dropping it.
const ROOT_FALLBACK_AGENT_ID: &str = "root";

impl ToolEventSink for SseToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        // Delegate to inner first so its side effects fire even if the
        // SSE send drops the event on a closed channel.
        if let Some(inner) = self.inner.as_ref() {
            inner.emit(event.clone());
        }
        let (agent_id, call_id, tool_name, args_preview, status, detail) =
            flatten_tool_lifecycle(event, AgentId(ROOT_FALLBACK_AGENT_ID.to_string()));
        // Skip terminal states — the subsequent `ToolResult` SSE event
        // conveys them with `output_preview` (see struct doc).
        if status != ToolCallStatus::Running {
            return;
        }
        let _ = self.tx.send(SseStreamEvent::ToolCall {
            agent_id: agent_id.0,
            call_id,
            tool_name,
            args_preview,
            status,
            detail,
        });
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/serverside/httpserver/sse_sink_test.rs"]
mod tests;
