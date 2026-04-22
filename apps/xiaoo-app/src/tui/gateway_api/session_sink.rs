use std::sync::{Arc, Mutex};

use agent_contracts::{LoopEventSink, ToolEventSink};
use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};

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
    fn on_turn_start(&self, _agent_id: &agent_types::common::ids::AgentId, _turn: u32) {}

    fn on_assistant_message(&self, agent_id: &agent_types::common::ids::AgentId, text: &str) {
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::SetAssistantContent {
                agent_id: agent_id.clone(),
                text: text.to_string(),
            });
    }

    fn on_tool_result(
        &self,
        agent_id: &agent_types::common::ids::AgentId,
        event: &ToolResultEvent,
    ) {
        let status = if event.is_error {
            ToolExecutionStatus::Failed
        } else {
            ToolExecutionStatus::Completed
        };
        let _ = self.updates_tx.send(SessionTurnUpdate::Tool {
            _agent_id: agent_id.clone(),
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

    fn on_loop_end(&self, _agent_id: &agent_types::common::ids::AgentId, summary: &LoopEndSummary) {
        if let Ok(mut stored) = self.loop_summary.lock() {
            *stored = Some(summary.clone());
        }
    }
}

impl ChannelToolEventSink {
    pub(super) fn new(updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>) -> Self {
        Self { updates_tx }
    }
}

impl ToolEventSink for ChannelToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
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
        };
        let _ = self.updates_tx.send(SessionTurnUpdate::Tool {
            // ToolEventSink has no agent context; tool lifecycle events are always
            // for the root agent and are never filtered by the TUI.
            _agent_id: agent_types::common::ids::AgentId(String::new()),
            update,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use agent_contracts::{LoopEventSink, ToolEventSink};
    use agent_types::common::ids::AgentId;
    use agent_types::events::{ToolLifecycleEvent, ToolResultEvent};
    use tokio::sync::mpsc::unbounded_channel;

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
}
