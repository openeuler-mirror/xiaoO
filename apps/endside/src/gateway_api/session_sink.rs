use std::sync::{Arc, Mutex};

use xiaoo_api::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};
use xiaoo_api::events::{LoopEventSink, ToolEventSink};

use crate::chat::{ToolExecutionStatus, ToolExecutionUpdate};

use super::session::{ChannelLoopEventSink, ChannelToolEventSink, SessionTurnUpdate};

impl ChannelLoopEventSink {
    pub(super) fn new(
        updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>,
        loop_summary: Arc<Mutex<Option<LoopEndSummary>>>,
    ) -> Self {
        Self {
            updates_tx,
            loop_summary,
        }
    }
}

impl LoopEventSink for ChannelLoopEventSink {
    fn on_turn_start(&self, agent_id: &xiaoo_api::chat::AgentId, turn: u32) {
        let _ = self.updates_tx.send(SessionTurnUpdate::TurnStart {
            agent_id: agent_id.clone(),
            turn,
        });
    }

    fn on_assistant_message(&self, agent_id: &xiaoo_api::chat::AgentId, text: &str) {
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::SetAssistantContent {
                agent_id: agent_id.clone(),
                text: text.to_string(),
            });
    }

    fn on_assistant_reasoning(&self, agent_id: &xiaoo_api::chat::AgentId, text: &str) {
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::SetAssistantThinking {
                agent_id: agent_id.clone(),
                text: text.to_string(),
            });
    }

    fn on_assistant_message_delta(&self, agent_id: &xiaoo_api::chat::AgentId, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::AppendAssistantContent {
                agent_id: agent_id.clone(),
                delta: delta.to_string(),
            });
    }

    fn on_assistant_reasoning_delta(&self, agent_id: &xiaoo_api::chat::AgentId, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::AppendAssistantThinking {
                agent_id: agent_id.clone(),
                delta: delta.to_string(),
            });
    }

    fn supports_message_delta(&self) -> bool {
        true
    }

    fn on_tool_result(&self, agent_id: &xiaoo_api::chat::AgentId, event: &ToolResultEvent) {
        let status = if event.is_error {
            ToolExecutionStatus::Failed
        } else {
            ToolExecutionStatus::Completed
        };
        let _ = self.updates_tx.send(SessionTurnUpdate::Tool {
            agent_id: agent_id.clone(),
            update: ToolExecutionUpdate {
                call_id: event.call_id.clone(),
                tool: event.tool_name.clone(),
                summary: String::new(),
                args_preview: event.args_preview.clone(),
                command_preview: None,
                command: None,
                detail: event.output_preview.clone(),
                status,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
        });
    }

    fn on_loop_end(&self, agent_id: &xiaoo_api::chat::AgentId, summary: &LoopEndSummary) {
        if let Ok(mut stored) = self.loop_summary.lock() {
            *stored = Some(summary.clone());
        }
        let _ = self.updates_tx.send(SessionTurnUpdate::LoopEnd {
            agent_id: agent_id.clone(),
            summary: summary.clone(),
        });
    }
}

impl ChannelToolEventSink {
    pub(super) fn new(updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>) -> Self {
        Self { updates_tx }
    }
}

impl ToolEventSink for ChannelToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        let (agent_id, update) =
            tool_lifecycle_update_from_event(event, xiaoo_api::chat::AgentId(String::new()));
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::Tool { agent_id, update });
    }
}

fn tool_lifecycle_update_from_event(
    event: ToolLifecycleEvent,
    fallback_agent_id: xiaoo_api::chat::AgentId,
) -> (xiaoo_api::chat::AgentId, ToolExecutionUpdate) {
    match event {
        ToolLifecycleEvent::AgentScoped { agent_id, event } => {
            tool_lifecycle_update_from_event(*event, agent_id)
        }
        event => {
            let update = match event {
                ToolLifecycleEvent::Pending {
                    call_id,
                    tool_name,
                    args_preview,
                }
                | ToolLifecycleEvent::Running {
                    call_id,
                    tool_name,
                    args_preview,
                } => ToolExecutionUpdate {
                    call_id,
                    tool: tool_name,
                    summary: String::new(),
                    args_preview,
                    command_preview: None,
                    command: None,
                    detail: String::new(),
                    status: ToolExecutionStatus::Running,
                    exit_code: None,
                    duration_ms: None,
                    file_change: None,
                },
                ToolLifecycleEvent::Completed {
                    call_id,
                    tool_name,
                    args_preview,
                } => ToolExecutionUpdate {
                    call_id,
                    tool: tool_name,
                    summary: String::new(),
                    args_preview,
                    command_preview: None,
                    command: None,
                    detail: String::new(),
                    status: ToolExecutionStatus::Completed,
                    exit_code: None,
                    duration_ms: None,
                    file_change: None,
                },
                ToolLifecycleEvent::Denied {
                    call_id,
                    tool_name,
                    reason,
                    args_preview,
                } => ToolExecutionUpdate {
                    call_id,
                    tool: tool_name,
                    summary: "denied by policy".to_string(),
                    args_preview,
                    command_preview: None,
                    command: None,
                    detail: reason.clone(),
                    status: ToolExecutionStatus::Failed,
                    exit_code: None,
                    duration_ms: None,
                    file_change: None,
                },
                ToolLifecycleEvent::Failed {
                    call_id,
                    tool_name,
                    error,
                    args_preview,
                } => ToolExecutionUpdate {
                    call_id,
                    tool: tool_name,
                    summary: "tool execution failed".to_string(),
                    args_preview,
                    command_preview: None,
                    command: None,
                    detail: error.clone(),
                    status: ToolExecutionStatus::Failed,
                    exit_code: None,
                    duration_ms: None,
                    file_change: None,
                },
                ToolLifecycleEvent::AgentScoped { .. } => unreachable!(),
            };
            (fallback_agent_id, update)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc::unbounded_channel;
    use xiaoo_api::chat::AgentId;
    use xiaoo_api::events::{LoopEventSink, ToolEventSink};
    use xiaoo_api::events::{ToolLifecycleEvent, ToolResultEvent};

    use super::{ChannelLoopEventSink, ChannelToolEventSink, SessionTurnUpdate};

    #[test]
    fn loop_tool_result_forwards_args_preview() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ChannelLoopEventSink::new(tx, Arc::new(Mutex::new(None)));

        sink.on_tool_result(
            &AgentId("root".to_string()),
            &ToolResultEvent {
                call_id: "call-1".to_string(),
                tool_name: "spawn_subagent".to_string(),
                output_preview: "{\"agent_id\":\"child\"}".to_string(),
                is_error: false,
                args_preview: "{\n  \"task_goal\": \"run\"\n}".to_string(),
            },
        );

        let SessionTurnUpdate::Tool { update, .. } = rx.try_recv().expect("tool update expected")
        else {
            panic!("expected tool update");
        };
        assert_eq!(update.args_preview, "{\n  \"task_goal\": \"run\"\n}");
        assert!(update.file_change.is_none());
    }

    #[test]
    fn lifecycle_tool_event_forwards_args_preview() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ChannelToolEventSink::new(tx);

        sink.emit(ToolLifecycleEvent::Running {
            call_id: "call-2".to_string(),
            tool_name: "join_subagent".to_string(),
            args_preview: "{\n  \"target_agent_id\": \"child\"\n}".to_string(),
        });

        let SessionTurnUpdate::Tool { update, .. } = rx.try_recv().expect("tool update expected")
        else {
            panic!("expected tool update");
        };
        assert_eq!(
            update.args_preview,
            "{\n  \"target_agent_id\": \"child\"\n}"
        );
        assert!(update.file_change.is_none());
    }

    #[test]
    fn scoped_lifecycle_tool_event_forwards_agent_id() {
        let (tx, mut rx) = unbounded_channel();
        let sink = ChannelToolEventSink::new(tx);

        sink.emit(
            ToolLifecycleEvent::Running {
                call_id: "call-child".to_string(),
                tool_name: "bash".to_string(),
                args_preview: "{}".to_string(),
            }
            .scoped(AgentId("child-agent".to_string())),
        );

        let SessionTurnUpdate::Tool {
            agent_id, update, ..
        } = rx.try_recv().expect("tool update expected")
        else {
            panic!("expected tool update");
        };
        assert_eq!(agent_id.0, "child-agent");
        assert_eq!(update.call_id, "call-child");
    }
}
