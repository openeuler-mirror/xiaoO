use std::sync::atomic::Ordering;

use crate::app_state::{AppState, InputMode};
use crate::chat::{Message, ToolExecutionStatus, ToolExecutionUpdate};
use crate::debug_log;
use crate::session_gateway::SessionTurnUpdate;

use super::runtime::{GatewayRuntime, PendingStreamDone, STREAM_REVEAL_CHARS_PER_TICK};

impl GatewayRuntime {
    pub fn poll_stream_updates(&mut self, state: &mut AppState) -> bool {
        let mut changed = false;
        while let Some(receiver) = &mut self.stream_rx {
            let update = match receiver.try_recv() {
                Ok(update) => update,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    self.handle_stream_disconnect(state);
                    changed = true;
                    break;
                }
            };
            changed = true;
            match update {
                SessionTurnUpdate::SetAssistantContent {
                    agent_id,
                    text: content,
                } => {
                    let root_agent_id =
                        super::runtime_request::resolve_agent_id(None, None, &state.agent_config)
                            .unwrap_or_default();
                    if agent_id.0 == root_agent_id || agent_id.0 == "cli-agent" {
                        self.stream_reveal_buffer.clear();
                        self.pending_stream_done = None;
                        self.set_stream_message_content(state, content, true);
                        self.record_first_token_latency_if_needed(state);
                        state.chat_state.stick_to_bottom = true;
                    }
                }
                SessionTurnUpdate::Tool {
                    _agent_id: _,
                    update,
                } => {
                    self.apply_tool_update(state, update);
                    state.chat_state.stick_to_bottom = true;
                }
                SessionTurnUpdate::InteractionPrompt(request) => {
                    if let Err(error) = state.open_interaction_prompt(request, true) {
                        tracing::warn!(error = %error, "TUI: failed to open interaction prompt");
                    }
                }
                SessionTurnUpdate::Done {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    estimated_input_tokens,
                    messages,
                } => {
                    self.pending_stream_done = Some(PendingStreamDone {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                        estimated_input_tokens,
                        messages,
                    });
                    self.stream_rx = None;
                }
                SessionTurnUpdate::Err(error) => {
                    self.stream_reveal_buffer.clear();
                    self.pending_stream_done = None;
                    self.set_stream_message_content(state, format!("Error: {}", error), false);
                    state.chat_state.is_loading = false;
                    self.stream_rx = None;
                    self.stream_message_index = None;
                    self.interaction_reply_tx = None;
                }
            }
        }

        let had_reveal_buffer = !self.stream_reveal_buffer.is_empty();
        self.reveal_stream_chars(state);
        changed |= had_reveal_buffer;

        if self.stream_reveal_buffer.is_empty() {
            if let Some(done) = self.pending_stream_done.take() {
                self.finish_stream_done(state, done);
                changed = true;
            }
        }

        changed
    }

    pub fn cancel_streaming(&mut self, state: &mut AppState) {
        if let Some(flag) = self.cancel_flag.take() {
            flag.store(true, Ordering::Relaxed);
        }
        let stream_message_index = self.stream_message_index.take();
        state.chat_state.is_loading = false;
        state.input_mode = InputMode::Editing;
        state.interaction_prompt = None;
        self.stream_rx = None;
        self.stream_reveal_buffer.clear();
        self.pending_stream_done = None;
        self.interaction_reply_tx = None;
        self.request_start = None;
        self.first_token_latency_recorded = false;
        if let Some(index) = stream_message_index {
            if let Some(message) = state.chat_state.messages.get_mut(index) {
                if message.is_streaming {
                    message.set_streaming(false);
                    if message.content.is_empty() {
                        message.set_content("[Cancelled]");
                    }
                }
            }
        } else if let Some(message) = state.chat_state.messages.iter_mut().rev().find(|message| {
            message.role == crate::chat::MessageRole::Assistant && message.is_streaming
        }) {
            message.set_streaming(false);
            if message.content.is_empty() {
                message.set_content("[Cancelled]");
            }
        }
        state.status_panel.update_metrics(0, 0, 0, 0);
    }

    fn stream_message_mut<'a>(
        &'a mut self,
        state: &'a mut AppState,
    ) -> Option<&'a mut crate::chat::Message> {
        let index = self.stream_message_index?;
        state.chat_state.messages.get_mut(index)
    }

    fn ensure_stream_message(&mut self, state: &mut AppState) {
        let has_valid_stream_message = self
            .stream_message_index
            .and_then(|index| state.chat_state.messages.get(index))
            .map(|message| message.role == crate::chat::MessageRole::Assistant)
            .unwrap_or(false);
        if has_valid_stream_message {
            return;
        }

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        self.stream_message_index = Some(state.chat_state.messages.len().saturating_sub(1));
    }

    fn set_stream_message_content(
        &mut self,
        state: &mut AppState,
        content: impl Into<String>,
        streaming: bool,
    ) {
        self.ensure_stream_message(state);
        if let Some(message) = self.stream_message_mut(state) {
            message.set_content(content);
            message.set_streaming(streaming);
        }
    }

    fn record_first_token_latency_if_needed(&mut self, state: &mut AppState) {
        if self.first_token_latency_recorded {
            return;
        }

        let Some(index) = self.stream_message_index else {
            return;
        };
        let has_content = state
            .chat_state
            .messages
            .get(index)
            .map(|message| !message.content.is_empty())
            .unwrap_or(false);
        if !has_content {
            return;
        }

        let Some(start) = self.request_start.as_ref() else {
            return;
        };
        state.status_panel.last_latency_ms = start.elapsed().as_millis() as u64;
        self.first_token_latency_recorded = true;
    }

    fn handle_stream_disconnect(&mut self, state: &mut AppState) {
        tracing::warn!("TUI: stream channel disconnected before Done/Err");

        let notice = "Error: 后台任务的流通道意外断开，任务可能仍在运行、已异常退出，或未正常发送完成信号。请检查日志；如需重新开始，请先按 Esc 结束当前状态。";
        let existing = self
            .stream_message_index
            .and_then(|index| state.chat_state.messages.get(index))
            .map(|message| message.content.trim().to_string())
            .unwrap_or_default();
        let content = if existing.is_empty() {
            notice.to_string()
        } else {
            format!("{existing}\n\n{notice}")
        };

        self.stream_reveal_buffer.clear();
        self.pending_stream_done = None;
        self.set_stream_message_content(state, content, false);
        state.chat_state.is_loading = false;
        self.stream_rx = None;
        self.stream_message_index = None;
        self.interaction_reply_tx = None;
        self.first_token_latency_recorded = false;
        state.status_panel.update_metrics(0, 0, 0, 0);
    }

    fn insert_aux_message(&mut self, state: &mut AppState, message: Message) {
        if let Some(index) = self.stream_message_index {
            state.chat_state.messages.insert(index, message);
            self.stream_message_index = Some(index + 1);
        } else {
            state.chat_state.messages.push(message);
        }
    }

    fn finalize_stream_message_before_aux(&mut self, state: &mut AppState) {
        let Some(index) = self.stream_message_index.take() else {
            return;
        };

        let remove_empty_message = state
            .chat_state
            .messages
            .get(index)
            .map(|message| {
                message.role == crate::chat::MessageRole::Assistant
                    && message.is_streaming
                    && message.content.trim().is_empty()
                    && message.thinking_content.trim().is_empty()
                    && message.tool_state.is_none()
                    && message.todo_state.is_none()
                    && message.completion_check_state.is_none()
            })
            .unwrap_or(false);

        if remove_empty_message {
            state.chat_state.messages.remove(index);
            return;
        }

        if let Some(message) = state.chat_state.messages.get_mut(index) {
            if message.role == crate::chat::MessageRole::Assistant && message.is_streaming {
                message.set_streaming(false);
            }
        }
    }

    fn apply_tool_update(&mut self, state: &mut AppState, update: ToolExecutionUpdate) {
        self.finalize_stream_message_before_aux(state);
        match update.status {
            ToolExecutionStatus::Running => {
                state.capture_tool_file_baseline(&update.call_id, &update.tool, &update.args_preview);
            }
            ToolExecutionStatus::Completed => {
                state.reconcile_tool_file_change_from_baseline(
                    &update.call_id,
                    update.file_change.clone(),
                );
            }
            ToolExecutionStatus::Failed => {
                state.discard_tool_file_baseline(&update.call_id);
                state.reconcile_tool_file_change(&update.call_id, update.file_change.clone());
            }
        }

        if let Some(existing) = state.chat_state.messages.iter_mut().find(|message| {
            message
                .tool_state
                .as_ref()
                .map(|tool| tool.call_id == update.call_id)
                .unwrap_or(false)
        }) {
            if let Some(tool) = existing.tool_state.as_mut() {
                tool.tool = update.tool;
                tool.summary = update.summary;
                tool.args_preview = update.args_preview;
                tool.command_preview = update.command_preview;
                tool.command = update.command;
                tool.detail = update.detail;
                tool.status = update.status;
                tool.exit_code = update.exit_code;
                tool.duration_ms = update.duration_ms;
            }
            existing.timestamp = chrono::Local::now();
            existing.mark_render_dirty();
            return;
        }

        self.insert_aux_message(state, Message::tool_event(update));
    }

    fn reveal_stream_chars(&mut self, state: &mut AppState) {
        if self.stream_reveal_buffer.is_empty() {
            return;
        }

        let split_index = self
            .stream_reveal_buffer
            .char_indices()
            .nth(STREAM_REVEAL_CHARS_PER_TICK)
            .map(|(index, _)| index)
            .unwrap_or(self.stream_reveal_buffer.len());
        let chunk: String = self.stream_reveal_buffer.drain(..split_index).collect();

        if let Some(message) = self.stream_message_mut(state) {
            message.append_content(&chunk);
        } else {
            self.stream_reveal_buffer.clear();
        }
    }

    fn finish_stream_done(&mut self, state: &mut AppState, done: PendingStreamDone) {
        if let Some(message) = self.stream_message_mut(state) {
            let response_content = message.content.clone();
            message.set_streaming(false);
            let response_preview = response_content.chars().take(120).collect::<String>();
            if response_content.len() > 120 {
                tracing::info!(
                    response_len = response_content.len(),
                    total_tokens = done.total_tokens,
                    response_preview = %format!("{}...", response_preview),
                    "TUI: gateway response done"
                );
            } else {
                tracing::info!(
                    response_len = response_content.len(),
                    total_tokens = done.total_tokens,
                    response_preview = %response_content,
                    "TUI: gateway response done"
                );
            }
            debug_log::debug_llm_log(&format!(
                "[TUI] Gateway response (total_tokens: {})",
                done.total_tokens
            ));
            debug_log::debug_llm_log_block("TUI LLM RESPONSE", &response_content);
        }
        state.session_messages = done.messages;
        state.chat_state.is_loading = false;
        self.stream_rx = None;
        self.stream_message_index = None;
        self.interaction_reply_tx = None;
        self.first_token_latency_recorded = false;
        if self.request_start.take().is_some() {
            let input_context_tokens = if done.estimated_input_tokens > 0 {
                done.estimated_input_tokens
            } else {
                done.prompt_tokens
            };
            state.status_panel.update_metrics(
                done.prompt_tokens,
                done.completion_tokens,
                state.status_panel.last_latency_ms,
                input_context_tokens,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use crate::app_state::AppState;
    use crate::chat::{Message, MessageRole, ToolExecutionStatus, ToolExecutionUpdate};

    use super::{GatewayRuntime, PendingStreamDone};

    fn test_state() -> AppState {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("test app state should initialize");
        state.chat_state.messages.clear();
        state
    }

    fn sample_tool_update(call_id: &str) -> ToolExecutionUpdate {
        ToolExecutionUpdate {
            call_id: call_id.to_string(),
            tool: "shell".to_string(),
            summary: "running".to_string(),
            args_preview: String::new(),
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Running,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        }
    }

    #[test]
    fn tool_update_preserves_previous_assistant_message() {
        let mut runtime = GatewayRuntime::new();
        let mut state = test_state();

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(0);
        runtime.set_stream_message_content(&mut state, "before tool", true);

        runtime.apply_tool_update(&mut state, sample_tool_update("call-1"));
        runtime.set_stream_message_content(&mut state, "after tool", true);

        assert_eq!(state.chat_state.messages.len(), 3);
        assert_eq!(state.chat_state.messages[0].role, MessageRole::Assistant);
        assert_eq!(state.chat_state.messages[0].content, "before tool");
        assert!(!state.chat_state.messages[0].is_streaming);

        let tool_state = state.chat_state.messages[1]
            .tool_state
            .as_ref()
            .expect("second message should be tool state");
        assert_eq!(tool_state.call_id, "call-1");

        assert_eq!(state.chat_state.messages[2].role, MessageRole::Assistant);
        assert_eq!(state.chat_state.messages[2].content, "after tool");
        assert!(state.chat_state.messages[2].is_streaming);
    }

    #[test]
    fn tool_update_drops_empty_streaming_placeholder() {
        let mut runtime = GatewayRuntime::new();
        let mut state = test_state();

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(0);

        runtime.apply_tool_update(&mut state, sample_tool_update("call-2"));

        assert_eq!(state.chat_state.messages.len(), 1);
        assert!(state.chat_state.messages[0].tool_state.is_some());
        assert!(runtime.stream_message_index.is_none());
    }

    #[test]
    fn tool_update_tracks_session_file_changes_by_call_id() {
        let mut runtime = GatewayRuntime::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");

        let mut state = AppState::new(PathBuf::from("config.toml"), workspace.clone())
            .expect("test app state should initialize");
        state.chat_state.messages.clear();

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "call-1".to_string(),
                tool: "file_edit".to_string(),
                summary: String::new(),
                args_preview: "{\n  \"file_path\": \"src/main.rs\"\n}".to_string(),
                command_preview: None,
                command: None,
                detail: String::new(),
                status: ToolExecutionStatus::Running,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
        );

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "call-1".to_string(),
                tool: "file_edit".to_string(),
                summary: String::new(),
                args_preview: "{\n  \"file_path\": \"src/main.rs\"\n}".to_string(),
                command_preview: None,
                command: None,
                detail: String::new(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
                file_change: Some(crate::chat::FileChangeDelta {
                    file_path: "src/main.rs".to_string(),
                    additions: 2,
                    deletions: 1,
                }),
            },
        );

        let stats = state
            .session_file_changes
            .get("src/main.rs")
            .expect("file stats should be tracked");
        assert_eq!(stats.additions, 2);
        assert_eq!(stats.deletions, 1);
    }

    #[test]
    fn first_token_latency_is_recorded_once_and_survives_completion() {
        let mut runtime = GatewayRuntime::new();
        let mut state = test_state();

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(0);
        runtime.request_start = Some(Instant::now() - Duration::from_millis(20));

        runtime.set_stream_message_content(&mut state, "H", true);
        runtime.record_first_token_latency_if_needed(&mut state);
        let first_token_latency_ms = state.status_panel.last_latency_ms;
        assert!(first_token_latency_ms >= 20);
        assert!(runtime.first_token_latency_recorded);

        runtime.request_start = Some(Instant::now() - Duration::from_millis(80));
        runtime.record_first_token_latency_if_needed(&mut state);
        assert_eq!(state.status_panel.last_latency_ms, first_token_latency_ms);

        runtime.finish_stream_done(
            &mut state,
            PendingStreamDone {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 42,
                estimated_input_tokens: 18,
                messages: Vec::new(),
            },
        );

        assert_eq!(state.status_panel.last_latency_ms, first_token_latency_ms);
        assert_eq!(state.status_panel.prompt_tokens, 10);
        assert_eq!(state.status_panel.completion_tokens, 5);
        assert_eq!(state.status_panel.input_context_tokens, 18);
    }

    #[test]
    fn completion_falls_back_to_prompt_tokens_when_estimate_is_missing() {
        let mut runtime = GatewayRuntime::new();
        let mut state = test_state();

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(0);
        runtime.request_start = Some(Instant::now());

        runtime.finish_stream_done(
            &mut state,
            PendingStreamDone {
                prompt_tokens: 24,
                completion_tokens: 6,
                total_tokens: 30,
                estimated_input_tokens: 0,
                messages: Vec::new(),
            },
        );

        assert_eq!(state.status_panel.input_context_tokens, 24);
    }
}
