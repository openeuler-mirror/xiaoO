use std::sync::atomic::Ordering;

use crate::app_state::{AppState, InputMode};
use crate::chat::{Message, ToolExecutionUpdate};
use crate::debug_log;
use crate::session_gateway::SessionTurnUpdate;

use super::runtime::{GatewayRuntime, PendingStreamDone, STREAM_REVEAL_CHARS_PER_TICK};

impl GatewayRuntime {
    pub fn poll_stream_updates(&mut self, state: &mut AppState) {
        while let Some(receiver) = &mut self.stream_rx {
            let update = match receiver.try_recv() {
                Ok(update) => update,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    self.handle_stream_disconnect(state);
                    break;
                }
            };
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
                        state.chat_state.stick_to_bottom = true;
                    }
                }
                SessionTurnUpdate::Tool {
                    agent_id: _,
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
                    messages,
                } => {
                    self.pending_stream_done = Some(PendingStreamDone {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
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

        self.reveal_stream_chars(state);

        if self.stream_reveal_buffer.is_empty() {
            if let Some(done) = self.pending_stream_done.take() {
                self.finish_stream_done(state, done);
            }
        }
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
        if let Some(index) = stream_message_index {
            if let Some(message) = state.chat_state.messages.get_mut(index) {
                if message.is_streaming {
                    message.is_streaming = false;
                    if message.content.is_empty() {
                        message.content = "[Cancelled]".to_string();
                    }
                }
            }
        } else if let Some(message) = state.chat_state.messages.iter_mut().rev().find(|message| {
            message.role == crate::chat::MessageRole::Assistant && message.is_streaming
        }) {
            message.is_streaming = false;
            if message.content.is_empty() {
                message.content = "[Cancelled]".to_string();
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

    fn set_stream_message_content(
        &mut self,
        state: &mut AppState,
        content: impl Into<String>,
        streaming: bool,
    ) {
        if let Some(message) = self.stream_message_mut(state) {
            message.content = content.into();
            message.is_streaming = streaming;
        }
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

    fn apply_tool_update(&mut self, state: &mut AppState, update: ToolExecutionUpdate) {
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
            message.content.push_str(&chunk);
        } else {
            self.stream_reveal_buffer.clear();
        }
    }

    fn finish_stream_done(&mut self, state: &mut AppState, done: PendingStreamDone) {
        if let Some(message) = self.stream_message_mut(state) {
            let response_content = message.content.clone();
            message.is_streaming = false;
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
        if let Some(start) = self.request_start.take() {
            let latency_ms = start.elapsed().as_millis() as u64;
            state.status_panel.update_metrics(
                done.prompt_tokens,
                done.completion_tokens,
                latency_ms,
                done.total_tokens,
            );
        }
    }
}
