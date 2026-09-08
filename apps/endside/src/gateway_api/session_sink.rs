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
    pub(super) fn new(
        updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>,
        workspace: std::path::PathBuf,
    ) -> Self {
        Self {
            updates_tx,
            workspace,
        }
    }

    /// Synchronously capture the pre-execution content of the tool's
    /// target file. `emit` runs on the executor thread right before
    /// `invoke_resolved_executor` (see `call_impl.rs`), so the file still
    /// holds its pre-tool state here. The TUI event loop drains the
    /// queued messages only after the tool has typically already
    /// finished, so a baseline read at `Tool`-update processing time
    /// freezes the post-execution content and content-diffs report 0/0
    /// (the race observed in error.log: BASELINE-INSERT reading the same
    /// content as the post-tool DUMP). Capture here and ship the payload
    /// ahead of the `Tool` update; the TUI injects it via
    /// `SessionTurnUpdate::ToolBaseline` (first-writer-wins, and the
    /// delayed `capture_tool_file_baseline` becomes a no-op for the same
    /// call).
    fn capture_baseline_update(&self, event: &ToolLifecycleEvent) -> Option<SessionTurnUpdate> {
        let event = match event {
            ToolLifecycleEvent::AgentScoped { event, .. } => event.as_ref(),
            event => event,
        };
        // Both Pending and Running precede executor invocation; capture on
        // the first one delivered so the payload is enqueued ahead of every
        // `Tool` update for this call.
        let (call_id, tool_name, args_preview) = match event {
            ToolLifecycleEvent::Pending {
                call_id,
                tool_name,
                args_preview,
            }
            | ToolLifecycleEvent::Running {
                call_id,
                tool_name,
                args_preview,
            } => (call_id, tool_name, args_preview),
            _ => return None,
        };
        let payload = xiaoo_shared::session_diff::capture_file_baseline_payload(
            &self.workspace,
            tool_name,
            args_preview,
        )?;
        Some(SessionTurnUpdate::ToolBaseline {
            call_id: call_id.clone(),
            payload,
        })
    }
}

impl ToolEventSink for ChannelToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        // Pre-execution baseline capture (see `capture_baseline_update`):
        // must be sent BEFORE the `Tool` update below so the TUI injects
        // the true "before" content first.
        if let Some(tool_baseline) = self.capture_baseline_update(&event) {
            let _ = self.updates_tx.send(tool_baseline);
        }
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
        let sink = ChannelToolEventSink::new(tx, std::path::PathBuf::from("."));

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
        let sink = ChannelToolEventSink::new(tx, std::path::PathBuf::from("."));

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

    /// Regression (local-mode race): for a file-targeting tool the sink
    /// must enqueue the pre-execution baseline BEFORE the `Tool` update
    /// so the TUI freezes the true "before" content even though it
    /// drains the channel after the tool already finished.
    #[test]
    fn file_tool_running_emits_baseline_before_tool_update() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/main.rs"), "before\n").unwrap();

        let (tx, mut rx) = unbounded_channel();
        let sink = ChannelToolEventSink::new(tx, workspace.clone());

        sink.emit(ToolLifecycleEvent::Running {
            call_id: "call-x".to_string(),
            tool_name: "file_edit".to_string(),
            args_preview: r#"{"file_path":"src/main.rs","old_string":"before","new_string":"after"}"#
                .to_string(),
        });

        let first = rx.try_recv().expect("first message expected");
        let SessionTurnUpdate::ToolBaseline { call_id, payload } = first else {
            panic!("expected ToolBaseline ahead of the Tool update");
        };
        assert_eq!(call_id, "call-x");
        assert_eq!(payload.file_path, "src/main.rs");
        assert_eq!(payload.content.as_deref(), Some("before\n"));

        let SessionTurnUpdate::Tool { update, .. } =
            rx.try_recv().expect("tool update expected")
        else {
            panic!("expected tool update");
        };
        assert_eq!(update.call_id, "call-x");
    }
}
