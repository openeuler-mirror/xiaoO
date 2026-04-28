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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseStreamEvent {
    TurnStart {
        agent_id: String,
        turn: u32,
    },
    TextDelta {
        delta: String,
        snapshot: String,
    },
    ToolResult {
        call_id: String,
        tool_name: String,
        output_preview: String,
        is_error: bool,
    },
    InteractionRequested {
        request: InteractionRequest,
    },
    Done {
        reply: String,
        raw_reply: String,
        conversation_id: String,
        session_id: String,
        turn_count: u32,
        total_tokens: usize,
        prompt_tokens: u64,
        completion_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<llm_client::ChatMessage>,
        stop_reason: String,
    },
    Error {
        error: String,
    },
    Cancelled {
        session_id: String,
    },
}

impl SseStreamEvent {
    fn event_name(&self) -> &'static str {
        match self {
            SseStreamEvent::TurnStart { .. } => "turn_start",
            SseStreamEvent::TextDelta { .. } => "text_delta",
            SseStreamEvent::ToolResult { .. } => "tool_result",
            SseStreamEvent::InteractionRequested { .. } => "interaction_requested",
            SseStreamEvent::Done { .. } => "done",
            SseStreamEvent::Error { .. } => "error",
            SseStreamEvent::Cancelled { .. } => "cancelled",
        }
    }
}

pub struct SseLoopEventSink {
    tx: mpsc::UnboundedSender<SseStreamEvent>,
    last_snapshot_len: Mutex<usize>,
    loop_summary: Mutex<Option<LoopEndSummary>>,
}

impl SseLoopEventSink {
    pub fn new(tx: mpsc::UnboundedSender<SseStreamEvent>) -> Self {
        Self {
            tx,
            last_snapshot_len: Mutex::new(0),
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
            *len = 0;
        }
        let _ = self.tx.send(SseStreamEvent::TurnStart {
            agent_id: agent_id.0.clone(),
            turn,
        });
    }

    fn on_assistant_message(&self, _agent_id: &AgentId, text: &str) {
        let delta = {
            let mut last_len = self
                .last_snapshot_len
                .lock()
                .expect("sse sink last_snapshot_len mutex should not be poisoned");
            let prev = *last_len;
            *last_len = text.len();
            if prev < text.len() {
                text[prev..].to_string()
            } else {
                return;
            }
        };
        let _ = self.tx.send(SseStreamEvent::TextDelta {
            delta,
            snapshot: text.to_string(),
        });
    }

    fn on_tool_result(&self, _agent_id: &AgentId, event: &ToolResultEvent) {
        let _ = self.tx.send(SseStreamEvent::ToolResult {
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
