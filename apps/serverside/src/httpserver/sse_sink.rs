use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use agent_contracts::{LoopEventSink, ToolEventSink};
use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};
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
        /// Args JSON preview for tool card rendering; backward compatible
        /// (older TUIs ignore unknown fields).
        #[serde(default)]
        args_preview: String,
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
    /// Tool lifecycle event forwarded so the remote TUI can drive the
    /// same tool-card state machine as local mode. `agent_id` routes
    /// to root message list or subagent lane.
    ToolCall {
        agent_id: String,
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args_preview: String,
        /// Lifecycle status carried over the wire. Mirrors the local
        /// `ToolExecutionStatus` enum.
        status: ToolCallStatus,
        /// Populated for `denied` / `failed` statuses (denial reason
        /// / executor error).
        #[serde(default)]
        detail: String,
    },
    /// Per-agent loop-end marker so the TUI clears `is_running` on the
    /// matching lane. Summary fields default to zero/empty for backward
    /// compat with older daemons.
    LoopEnd {
        agent_id: String,
        #[serde(default)]
        turn_count: u32,
        #[serde(default)]
        total_tokens: usize,
        #[serde(default)]
        stop_reason: String,
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

/// Wire-format mirror of `agent_types::tool::ToolExecutionStatus`,
/// kept independent so the daemon ↔ TUI contract survives TUI renames.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Tool args received, executor about to run.
    Running,
    /// Executor returned successfully.
    Completed,
    /// Executor returned an error.
    Failed,
    /// Tool args were rejected by the policy layer before execution.
    Denied,
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
            SseStreamEvent::ToolCall { .. } => "tool_call",
            SseStreamEvent::LoopEnd { .. } => "loop_end",
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

    #[test]
    fn tool_call_event_serializes_snake_case_status() {
        let value = serde_json::to_value(SseStreamEvent::ToolCall {
            agent_id: "child-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            args_preview: "{}".to_string(),
            status: ToolCallStatus::Running,
            detail: String::new(),
        })
        .expect("event should serialize");

        assert_eq!(value["type"], "tool_call");
        assert_eq!(value["status"], "running");
        assert_eq!(value["agent_id"], "child-1");
    }

    #[test]
    fn loop_end_event_carries_agent_id() {
        let value = serde_json::to_value(SseStreamEvent::LoopEnd {
            agent_id: "child-1".to_string(),
            turn_count: 3,
            total_tokens: 1024,
            stop_reason: "end_turn".to_string(),
        })
        .expect("event should serialize");

        assert_eq!(value["type"], "loop_end");
        assert_eq!(value["agent_id"], "child-1");
        assert_eq!(value["turn_count"], 3);
        assert_eq!(value["total_tokens"], 1024);
        assert_eq!(value["stop_reason"], "end_turn");
    }

    #[test]
    fn tool_result_event_carries_args_preview() {
        let value = serde_json::to_value(SseStreamEvent::ToolResult {
            agent_id: "root".to_string(),
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            output_preview: "done".to_string(),
            is_error: false,
            args_preview: "{\"command\":\"ls\"}".to_string(),
        })
        .expect("event should serialize");

        assert_eq!(value["type"], "tool_result");
        assert_eq!(value["args_preview"], "{\"command\":\"ls\"}");
    }

    /// Inner sink should still receive the event after the wrapper
    /// forwards it to SSE.
    #[test]
    fn sse_tool_event_sink_delegates_to_inner() {
        #[derive(Default)]
        struct Recorder {
            seen: Mutex<Vec<ToolLifecycleEvent>>,
        }
        impl ToolEventSink for Recorder {
            fn emit(&self, event: ToolLifecycleEvent) {
                self.seen.lock().unwrap().push(event);
            }
        }

        let (tx, _rx) = mpsc::unbounded_channel::<SseStreamEvent>();
        let recorder = Arc::new(Recorder::default());
        let sink = SseToolEventSink::with_inner(tx, recorder.clone() as Arc<dyn ToolEventSink>);
        sink.emit(ToolLifecycleEvent::Running {
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            args_preview: "{}".to_string(),
        });

        let seen = recorder.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(matches!(seen[0], ToolLifecycleEvent::Running { .. }));
    }

    /// Drives a subagent lifecycle sequence (`tool_call(running)` ->
    /// `tool_call(completed)` [skipped: terminal] -> `loop_end`)
    /// through `SseToolEventSink` + `SseLoopEventSink` and asserts the
    /// SSE channel receives the events in order. Terminal lifecycle
    /// events (`Completed`) are NOT forwarded as `ToolCall` SSE events
    /// — the subsequent `ToolResult` SSE event conveys the terminal
    /// state, avoiding duplicate updates.
    #[tokio::test]
    async fn sse_subagent_lifecycle_events_reach_channel_in_order() {
        let (tx, mut rx) = mpsc::unbounded_channel::<SseStreamEvent>();
        // Inner sink is a no-op; diff-tracker delegation is covered
        // separately by `sse_tool_event_sink_delegates_to_inner`.
        struct NoopSink;
        impl ToolEventSink for NoopSink {
            fn emit(&self, _event: ToolLifecycleEvent) {}
        }

        let loop_sink = SseLoopEventSink::new(tx.clone());
        let tool_sink =
            SseToolEventSink::with_inner(tx, Arc::new(NoopSink) as Arc<dyn ToolEventSink>);

        // tool call (running) -> tool call (completed, skipped) -> loop end
        // mirrors a real subagent turn on the wire.
        tool_sink.emit(
            ToolLifecycleEvent::Running {
                call_id: "child-call-1".to_string(),
                tool_name: "bash".to_string(),
                args_preview: r#"{"command":"ls"}"#.to_string(),
            }
            .scoped(AgentId("child-1".to_string())),
        );
        tool_sink.emit(
            ToolLifecycleEvent::Completed {
                call_id: "child-call-1".to_string(),
                tool_name: "bash".to_string(),
                args_preview: r#"{"command":"ls"}"#.to_string(),
            }
            .scoped(AgentId("child-1".to_string())),
        );
        loop_sink.on_loop_end(
            &AgentId("child-1".to_string()),
            &LoopEndSummary {
                turn_count: 1,
                total_tokens: 0,
                stop_reason: "end_turn".to_string(),
            },
        );

        // Drain the channel and assert the wire-format sequence.
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            received.push(event);
        }
        // Expect: tool_call(running) + loop_end. The `Completed`
        // lifecycle event is NOT forwarded as SSE (terminal state is
        // conveyed by the subsequent `ToolResult` SSE event).
        assert_eq!(
            received.len(),
            2,
            "expected running + loop_end (completed is skipped); got {received:?}"
        );
        assert!(matches!(
            received[0],
            SseStreamEvent::ToolCall {
                ref agent_id,
                status: ToolCallStatus::Running,
                ..
            } if agent_id == "child-1"
        ));
        assert!(matches!(
            received[1],
            SseStreamEvent::LoopEnd {
                ref agent_id,
                turn_count: 1,
                total_tokens: 0,
                ref stop_reason,
            } if agent_id == "child-1" && stop_reason == "end_turn"
        ));
    }
}
