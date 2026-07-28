use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Mutex;

use agent_contracts::LoopEventSink;
use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use agent_types::interaction::InteractionRequest;
use axum::response::sse;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use xiaoo_shared::plan::{
    PlanForwarder, SpawnSubagentMetadata, SubagentMetaForwarder, TodoSnapshotItem,
    TodoSnapshotUpdate,
};
use xiaoo_shared::session_diff::{FileChangeDelta, SessionDiffForwarder};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseStreamEvent {
    TurnStart {
        agent_id: String,
        turn: u32,
    },
    TextDelta {
        agent_id: String,
        delta: String,
        snapshot: String,
    },
    ThinkingDelta {
        agent_id: String,
        delta: String,
        snapshot: String,
    },
    ToolResult {
        agent_id: String,
        call_id: String,
        tool_name: String,
        output_preview: String,
        is_error: bool,
    },
    /// Per-call file change delta computed by the daemon's
    /// `SessionDiffTracker`. Forwarded to the TUI so the remote-mode session
    /// diff panel mirrors the local-mode computation exactly.
    ToolFileChange {
        call_id: String,
        file_path: String,
        additions: u32,
        deletions: u32,
    },
    /// Plan snapshot parsed by the daemon from the `todo_write` tool's args.
    /// Forwarded to the TUI so the remote-mode plan panel mirrors the
    /// local-mode computation exactly.
    PlanUpdate {
        title: String,
        items: Vec<TodoSnapshotItem>,
    },
    /// Subagent lane metadata parsed by the daemon from the `spawn_subagent`
    /// tool's args + output. Forwarded to the TUI so the remote-mode
    /// subagent lanes mirror the local-mode computation exactly.
    SubagentSpawn {
        agent_id: String,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
    },
    InteractionRequested {
        request: InteractionRequest,
    },
    Done {
        reply: String,
        raw_reply: String,
        conversation_id: String,
        #[serde(rename = "runtime_id")]
        session_id: String,
        turn_count: u32,
        total_tokens: usize,
        prompt_tokens: u64,
        completion_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<llm_client::ChatMessage>,
        stop_reason: String,
        #[serde(default)]
        actions: Vec<agent_types::hook::HookAction>,
    },
    Error {
        error: String,
    },
    Cancelled {
        #[serde(rename = "runtime_id")]
        session_id: String,
    },
}

impl SseStreamEvent {
    fn event_name(&self) -> &'static str {
        match self {
            SseStreamEvent::TurnStart { .. } => "turn_start",
            SseStreamEvent::TextDelta { .. } => "text_delta",
            SseStreamEvent::ThinkingDelta { .. } => "thinking_delta",
            SseStreamEvent::ToolResult { .. } => "tool_result",
            SseStreamEvent::ToolFileChange { .. } => "tool_file_change",
            SseStreamEvent::PlanUpdate { .. } => "plan_update",
            SseStreamEvent::SubagentSpawn { .. } => "subagent_spawn",
            SseStreamEvent::InteractionRequested { .. } => "interaction_requested",
            SseStreamEvent::Done { .. } => "done",
            SseStreamEvent::Error { .. } => "error",
            SseStreamEvent::Cancelled { .. } => "cancelled",
        }
    }
}

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

    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent) {
        let _ = self.tx.send(SseStreamEvent::ToolResult {
            agent_id: agent_id.0.clone(),
            call_id: event.call_id.clone(),
            tool_name: event.tool_name.clone(),
            output_preview: event.output_preview.clone(),
            is_error: event.is_error,
        });
    }

    fn on_loop_end(&self, _agent_id: &AgentId, summary: &LoopEndSummary) {
        if let Ok(mut stored) = self.loop_summary.lock() {
            *stored = Some(summary.clone());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_event_serializes_runtime_id() {
        let value = serde_json::to_value(SseStreamEvent::Cancelled {
            session_id: "runtime-1".to_string(),
        })
        .expect("event should serialize");

        assert_eq!(value["runtime_id"], "runtime-1");
        assert!(value.get("session_id").is_none());
    }
}
