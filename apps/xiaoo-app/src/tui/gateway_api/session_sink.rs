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
            agent_id: agent_id.clone(),
            update: ToolExecutionUpdate {
                call_id: event.call_id.clone(),
                tool: event.tool_name.clone(),
                summary: String::new(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: event.output_preview.clone(),
                status,
                exit_code: None,
                duration_ms: None,
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
            ToolLifecycleEvent::Pending { call_id, tool_name }
            | ToolLifecycleEvent::Running { call_id, tool_name } => ToolExecutionUpdate {
                call_id,
                tool: tool_name,
                summary: String::new(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: String::new(),
                status: ToolExecutionStatus::Running,
                exit_code: None,
                duration_ms: None,
            },
            ToolLifecycleEvent::Completed { call_id, tool_name } => ToolExecutionUpdate {
                call_id,
                tool: tool_name,
                summary: String::new(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: String::new(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
            },
            ToolLifecycleEvent::Denied {
                call_id,
                tool_name,
                reason,
            } => ToolExecutionUpdate {
                call_id,
                tool: tool_name,
                summary: "denied by policy".to_string(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: reason.clone(),
                status: ToolExecutionStatus::Failed,
                exit_code: None,
                duration_ms: None,
            },
            ToolLifecycleEvent::Failed {
                call_id,
                tool_name,
                error,
            } => ToolExecutionUpdate {
                call_id,
                tool: tool_name,
                summary: "tool execution failed".to_string(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: error.clone(),
                status: ToolExecutionStatus::Failed,
                exit_code: None,
                duration_ms: None,
            },
        };
        let _ = self.updates_tx.send(SessionTurnUpdate::Tool {
            // ToolEventSink has no agent context; tool lifecycle events are always
            // for the root agent and are never filtered by the TUI.
            agent_id: agent_types::common::ids::AgentId(String::new()),
            update,
        });
    }
}
