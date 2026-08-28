use crate::app_state::{AppState, InputMode};
use crate::chat::{Message, TodoSnapshotUpdate, ToolExecutionStatus, ToolExecutionUpdate};
use crate::debug_log;
use crate::session_gateway::SessionTurnUpdate;
use xiaoo_api::chat::AgentId;
use xiaoo_shared::plan::todo_snapshot_from_tool_args;

use super::runtime::{GatewayRuntime, PendingStreamDone, STREAM_REVEAL_CHARS_PER_TICK};

impl GatewayRuntime {
    pub fn poll_stream_updates(&mut self, state: &mut AppState) -> bool {
        let mut changed = false;
        if self.remote.is_none() {
            if let Some(health) = self.session_gateway.take_memory_health_update() {
                state.status_panel.memory_status = match health {
                    crate::gateway::MemoryAutomationHealth::Healthy => {
                        crate::status_panel::MemoryStatus::Connected
                    }
                    crate::gateway::MemoryAutomationHealth::Degraded => {
                        crate::status_panel::MemoryStatus::Degraded
                    }
                };
                changed = true;
            }
        }
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
                SessionTurnUpdate::TurnStart { agent_id, turn } => {
                    if !is_root_stream_agent(&agent_id, state) {
                        // Use the preserve-metadata variant so a `SubagentSpawn`
                        // SSE event's title is not clobbered by the generic
                        // "Subagent xxx" fallback.
                        let lane = state.chat_state.ensure_subagent_lane_preserve_metadata(
                            agent_id.0.clone(),
                            None,
                            format!("Subagent {}", short_agent_id(&agent_id.0)),
                            String::new(),
                            String::new(),
                        );
                        lane.is_running = true;
                        lane.last_turn = Some(turn);
                    } else {
                        // Root agent entering a new turn: finalize the
                        // previous turn's stream message (if it has
                        // content) so the next `TextDelta` creates a new
                        // message instead of replacing the previous
                        // turn's content. The `has_content` guard
                        // preserves the empty placeholder message (loading
                        // indicator) created before the first `TextDelta`.
                        let has_content = self
                            .stream_message_index
                            .and_then(|index| state.chat_state.messages.get(index))
                            .map_or(false, |message| {
                                !message.content.trim().is_empty()
                                    || !message.thinking_content.trim().is_empty()
                            });
                        if has_content {
                            self.finalize_stream_message_before_aux(state);
                        }
                    }
                }
                SessionTurnUpdate::SetAssistantContent {
                    agent_id,
                    text: content,
                } => {
                    if is_root_stream_agent(&agent_id, state) {
                        self.stream_reveal_buffer.clear();
                        self.pending_stream_done = None;
                        self.set_stream_message_content(state, content, true);
                        self.record_first_token_latency_if_needed(state);
                    } else {
                        self.set_subagent_stream_message_content(state, &agent_id.0, content, true);
                    }
                }
                SessionTurnUpdate::SetAssistantThinking {
                    agent_id,
                    text: content,
                } => {
                    if is_root_stream_agent(&agent_id, state) {
                        self.set_stream_message_thinking_content(state, content, true);
                        self.record_first_token_latency_if_needed(state);
                    } else {
                        self.set_subagent_stream_message_thinking_content(
                            state,
                            &agent_id.0,
                            content,
                            true,
                        );
                    }
                }
                SessionTurnUpdate::Tool { agent_id, update } => {
                    if is_root_stream_agent(&agent_id, state) {
                        self.apply_tool_update(state, update, Some(agent_id.0));
                    } else {
                        self.apply_subagent_tool_update(state, &agent_id.0, update);
                    }
                }
                SessionTurnUpdate::ToolFileChange { call_id, delta } => {
                    state.apply_remote_delta(&call_id, delta);
                }
                SessionTurnUpdate::PlanUpdate { snapshot } => {
                    self.apply_todo_snapshot(state, snapshot);
                }
                SessionTurnUpdate::SubagentSpawn { metadata } => {
                    state.chat_state.ensure_subagent_lane(
                        metadata.agent_id,
                        metadata.parent_agent_id,
                        metadata.title,
                        metadata.description,
                        metadata.task_goal,
                    );
                }
                SessionTurnUpdate::LoopEnd { agent_id, summary } => {
                    let _ = summary.turn_count;
                    if !is_root_stream_agent(&agent_id, state) {
                        if let Some(lane) = state.chat_state.subagent_lanes.get_mut(&agent_id.0) {
                            lane.is_running = false;
                            if let Some(index) = lane.stream_message_index.take() {
                                if let Some(message) = lane.messages.get_mut(index) {
                                    if message.role == crate::chat::MessageRole::Assistant
                                        && message.is_streaming
                                    {
                                        message.set_streaming(false);
                                    }
                                }
                            }
                        }
                    }
                    // Drop per-call state so the tracker's per-call maps do
                    // not grow unboundedly across turns; per-file totals and
                    // session-start baselines are retained.
                    state.diff_tracker.clear_per_turn_state();
                }
                SessionTurnUpdate::InteractionPrompt(request) => {
                    if let Err(error) = state.open_interaction_prompt(request, true) {
                        tracing::warn!(error = %error, "TUI: failed to open interaction prompt");
                    }
                }
                SessionTurnUpdate::PendingUserMessagesConsumed { prompts } => {
                    for prompt in prompts {
                        state.chat_state.remove_pending_turn_prompt(&prompt);
                        self.insert_aux_message(state, Message::user(prompt));
                    }
                    state.chat_state.stick_to_bottom = true;
                }
                SessionTurnUpdate::MemoryStatus(memory_status) => {
                    state.status_panel.memory_status = memory_status;
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
                SessionTurnUpdate::HookActions(actions) => {
                    if !actions.is_empty() {
                        self.pending_hook_actions.extend(actions);
                    }
                }
                SessionTurnUpdate::Err(error) => {
                    let display_error = crate::error_log::record_tui_error("remote_input", &error);
                    self.stream_reveal_buffer.clear();
                    self.pending_stream_done = None;
                    self.set_stream_message_content(state, display_error, false);
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
        if self.remote.is_some() {
            self.cancel_remote_turn(state.session_id.clone());
        }
        // Fire the shared CancellationToken so the backend's agent loop
        // observes cancellation via `ctx.state.cancel.is_cancelled()` and
        // exits through `LoopDecision::ReturnCancelled` — which returns
        // `Ok(Complete(..))`, letting `persist_lane_state` save the partial
        // loop state (user prompt + assistant reply + tool results) so the
        // next turn has full context.
        if let Some(token) = self.cancel_token.take() {
            token.cancel();
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
        state.status_panel.update_metrics(0, 0, 0, 0, false);
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

    fn set_stream_message_thinking_content(
        &mut self,
        state: &mut AppState,
        content: impl Into<String>,
        streaming: bool,
    ) {
        self.ensure_stream_message(state);
        if let Some(message) = self.stream_message_mut(state) {
            message.set_thinking_content(content);
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
            .map(|message| !message.content.is_empty() || !message.thinking_content.is_empty())
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
        state.status_panel.update_metrics(0, 0, 0, 0, false);
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

    fn apply_tool_update(
        &mut self,
        state: &mut AppState,
        update: ToolExecutionUpdate,
        parent_agent_id: Option<String>,
    ) {
        if update.tool == "todo_write" {
            if update.status == ToolExecutionStatus::Completed {
                if let Some(todo_update) = todo_snapshot_from_tool_args(&update.args_preview) {
                    self.apply_todo_snapshot(state, todo_update);
                    return;
                }
            } else if update.status == ToolExecutionStatus::Running {
                return;
            }
        }

        self.ensure_spawned_subagent_lane_from_tool_update(state, parent_agent_id.clone(), &update);
        self.finalize_stream_message_before_aux(state);
        match update.status {
            ToolExecutionStatus::Running => {
                state.on_tool_running(&update.call_id, &update.tool, &update.args_preview);
            }
            ToolExecutionStatus::Completed => {
                let fallback_file_change = update.file_change.clone().or_else(|| {
                    crate::app_state::file_change_delta_from_tool_args(
                        &update.tool,
                        &update.args_preview,
                    )
                });
                state.on_tool_completed(
                    &update.call_id,
                    &update.tool,
                    &update.args_preview,
                    fallback_file_change,
                );
            }
            ToolExecutionStatus::Failed => {
                state.on_tool_failed(&update.call_id, update.file_change.clone());
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

    fn ensure_spawned_subagent_lane_from_tool_update(
        &mut self,
        state: &mut AppState,
        parent_agent_id: Option<String>,
        update: &ToolExecutionUpdate,
    ) {
        if update.tool != "spawn_subagent" || update.status != ToolExecutionStatus::Completed {
            return;
        }
        let Some(agent_id) = parse_spawn_subagent_agent_id_from_detail(&update.detail) else {
            return;
        };
        // Remote mode: `args_preview` is stripped, so parsing yields `None`.
        // The lane was already populated by the earlier `SubagentSpawn` SSE
        // event; use the preserve-metadata variant to avoid clobbering it.
        let Some(metadata) = parse_spawn_subagent_metadata_from_args(&update.args_preview) else {
            state.chat_state.ensure_subagent_lane_preserve_metadata(
                agent_id.clone(),
                parent_agent_id,
                format!("Subagent {}", short_agent_id(&agent_id)),
                String::new(),
                String::new(),
            );
            return;
        };
        state.chat_state.ensure_subagent_lane(
            agent_id.clone(),
            parent_agent_id,
            metadata
                .title
                .unwrap_or_else(|| format!("Subagent {}", short_agent_id(&agent_id))),
            metadata.description.unwrap_or_default(),
            metadata.task_goal.unwrap_or_default(),
        );
    }

    fn ensure_subagent_stream_message(&mut self, state: &mut AppState, agent_id: &str) {
        let lane = state.chat_state.ensure_subagent_lane_preserve_metadata(
            agent_id.to_string(),
            None,
            format!("Subagent {}", short_agent_id(agent_id)),
            String::new(),
            String::new(),
        );
        let has_valid_stream_message = lane
            .stream_message_index
            .and_then(|index| lane.messages.get(index))
            .map(|message| message.role == crate::chat::MessageRole::Assistant)
            .unwrap_or(false);
        if has_valid_stream_message {
            return;
        }
        lane.messages.push(Message::assistant_streaming());
        lane.stream_message_index = Some(lane.messages.len().saturating_sub(1));
        lane.is_running = true;
    }

    fn subagent_stream_message_mut<'a>(
        &mut self,
        state: &'a mut AppState,
        agent_id: &str,
    ) -> Option<&'a mut crate::chat::Message> {
        let lane = state.chat_state.subagent_lanes.get_mut(agent_id)?;
        let index = lane.stream_message_index?;
        lane.messages.get_mut(index)
    }

    fn set_subagent_stream_message_content(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
        content: impl Into<String>,
        streaming: bool,
    ) {
        self.ensure_subagent_stream_message(state, agent_id);
        if let Some(message) = self.subagent_stream_message_mut(state, agent_id) {
            message.set_content(content);
            message.set_streaming(streaming);
        }
    }

    fn set_subagent_stream_message_thinking_content(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
        content: impl Into<String>,
        streaming: bool,
    ) {
        self.ensure_subagent_stream_message(state, agent_id);
        if let Some(message) = self.subagent_stream_message_mut(state, agent_id) {
            message.set_thinking_content(content);
            message.set_streaming(streaming);
        }
    }

    fn insert_subagent_aux_message(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
        message: Message,
    ) {
        let lane = state.chat_state.ensure_subagent_lane_preserve_metadata(
            agent_id.to_string(),
            None,
            format!("Subagent {}", short_agent_id(agent_id)),
            String::new(),
            String::new(),
        );
        if let Some(index) = lane.stream_message_index {
            lane.messages.insert(index, message);
            lane.stream_message_index = Some(index + 1);
        } else {
            lane.messages.push(message);
        }
    }

    fn finalize_subagent_stream_message_before_aux(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
    ) {
        let Some(lane) = state.chat_state.subagent_lanes.get_mut(agent_id) else {
            return;
        };
        let Some(index) = lane.stream_message_index.take() else {
            return;
        };

        let remove_empty_message = lane
            .messages
            .get(index)
            .map(|message| {
                message.role == crate::chat::MessageRole::Assistant
                    && message.is_streaming
                    && message.content.trim().is_empty()
                    && message.thinking_content.trim().is_empty()
                    && message.tool_state.is_none()
                    && message.completion_check_state.is_none()
            })
            .unwrap_or(false);

        if remove_empty_message {
            lane.messages.remove(index);
            return;
        }

        if let Some(message) = lane.messages.get_mut(index) {
            if message.role == crate::chat::MessageRole::Assistant && message.is_streaming {
                message.set_streaming(false);
            }
        }
    }

    fn apply_subagent_tool_update(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
        update: ToolExecutionUpdate,
    ) {
        self.ensure_spawned_subagent_lane_from_tool_update(
            state,
            Some(agent_id.to_string()),
            &update,
        );
        self.finalize_subagent_stream_message_before_aux(state, agent_id);
        match update.status {
            ToolExecutionStatus::Running => {
                state.on_tool_running(&update.call_id, &update.tool, &update.args_preview);
            }
            ToolExecutionStatus::Completed => {
                let fallback_file_change = update.file_change.clone().or_else(|| {
                    crate::app_state::file_change_delta_from_tool_args(
                        &update.tool,
                        &update.args_preview,
                    )
                });
                state.on_tool_completed(
                    &update.call_id,
                    &update.tool,
                    &update.args_preview,
                    fallback_file_change,
                );
            }
            ToolExecutionStatus::Failed => {
                state.on_tool_failed(&update.call_id, update.file_change.clone());
            }
        }

        let lane = state.chat_state.ensure_subagent_lane_preserve_metadata(
            agent_id.to_string(),
            None,
            format!("Subagent {}", short_agent_id(agent_id)),
            String::new(),
            String::new(),
        );
        if let Some(existing) = lane.messages.iter_mut().find(|message| {
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

        self.insert_subagent_aux_message(state, agent_id, Message::tool_event(update));
    }

    fn apply_todo_snapshot(&mut self, state: &mut AppState, update: TodoSnapshotUpdate) {
        // `show_sidebar` in the root layout cache depends on
        // `plan_state.is_some()` (see `App::ui`), so a Some<->None transition
        // must invalidate the cached layout split. Content-only updates
        // (still `Some`) don't change sidebar visibility, so the cache stays.
        let plan_presence_changed = state.plan_state.is_some() != !update.items.is_empty();
        state.plan_state = if update.items.is_empty() {
            None
        } else {
            Some(crate::chat::TodoMessageState {
                title: update.title,
                items: update
                    .items
                    .into_iter()
                    .map(|item| (item.status, item.content))
                    .collect(),
            })
        };
        if plan_presence_changed {
            state.render_state.cached_area = None;
        }
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
            let (input_context_tokens, input_context_tokens_estimated) = if done.prompt_tokens > 0 {
                (done.prompt_tokens, false)
            } else if done.estimated_input_tokens > 0 {
                (done.estimated_input_tokens, true)
            } else {
                (0, false)
            };
            state.status_panel.update_metrics(
                done.prompt_tokens,
                done.completion_tokens,
                state.status_panel.last_latency_ms,
                input_context_tokens,
                input_context_tokens_estimated,
            );
        }
        // Remote mode: SSE emits `Done` but no `LoopEnd`, so the per-turn
        // cleanup wired into `LoopEnd` never runs. Drop per-call state here;
        // per-file totals are retained. In local mode this is a redundant
        // no-op since `LoopEnd` already cleared the maps.
        state.diff_tracker.clear_per_turn_state();
    }
}

fn is_root_stream_agent(agent_id: &AgentId, state: &AppState) -> bool {
    if agent_id.0.is_empty() || agent_id.0 == "cli-agent" {
        return true;
    }
    super::runtime_request::resolve_agent_id(None, None, &state.agent_config)
        .map(|root_agent_id| agent_id.0 == root_agent_id)
        .unwrap_or(false)
}

struct SpawnSubagentMetadata {
    title: Option<String>,
    description: Option<String>,
    task_goal: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SpawnSubagentArgs {
    #[serde(default)]
    description: String,
    #[serde(default)]
    task_goal: String,
    #[serde(default)]
    task_context: String,
    #[serde(default)]
    subagent_role_id: Option<String>,
}

fn parse_spawn_subagent_agent_id_from_detail(detail: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(detail.trim()).ok()?;
    value.get("agent_id")?.as_str().map(ToOwned::to_owned)
}

fn parse_spawn_subagent_metadata_from_args(args_preview: &str) -> Option<SpawnSubagentMetadata> {
    let args: SpawnSubagentArgs = serde_json::from_str(args_preview).ok()?;
    let description = non_empty(args.description);
    let task_goal = non_empty(args.task_goal);
    let task_context = non_empty(args.task_context);
    let title = description
        .clone()
        .or_else(|| {
            task_goal
                .as_deref()
                .and_then(first_non_empty_line)
                .map(str::to_string)
        })
        .or_else(|| args.subagent_role_id.map(|role| format!("Subagent {role}")));
    Some(SpawnSubagentMetadata {
        title,
        description: description.or(task_context),
        task_goal,
    })
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn first_non_empty_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

fn short_agent_id(agent_id: &str) -> String {
    let trimmed = agent_id.trim();
    if trimmed.chars().count() <= 8 {
        trimmed.to_string()
    } else {
        trimmed.chars().take(8).collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use tokio::sync::{mpsc, watch};
    use xiaoo_api::chat::AgentId;

    use crate::app_state::AppState;
    use crate::chat::{
        Message, MessageRole, TodoDisplayStatus, ToolExecutionStatus, ToolExecutionUpdate,
    };
    use crate::gateway::MemoryAutomationHealth;
    use crate::session_gateway::SessionTurnUpdate;
    use crate::status_panel::MemoryStatus;

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
    fn todo_write_completed_update_updates_right_panel_plan() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "todo-1".to_string(),
                tool: "todo_write".to_string(),
                summary: String::new(),
                args_preview: serde_json::json!({
                    "todos": [
                        { "content": "Inspect current implementation", "status": "completed" },
                        { "content": "Add todo_write tool", "status": "in_progress" }
                    ]
                })
                .to_string(),
                command_preview: None,
                command: None,
                detail: String::new(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
            None,
        );

        assert!(state.chat_state.messages.is_empty());
        let todo = state
            .plan_state
            .as_ref()
            .expect("todo snapshot should update right panel plan");
        assert_eq!(todo.items.len(), 2);
        assert_eq!(todo.items[0].0, TodoDisplayStatus::Completed);
        assert_eq!(todo.items[1].0, TodoDisplayStatus::InProgress);
    }

    /// Regression: `apply_todo_snapshot` must invalidate the root layout cache
    /// (`cached_area`) on a Some<->None `plan_state` transition, because
    /// sidebar visibility depends on `plan_state.is_some()` (see `App::ui`).
    /// Content-only updates (still `Some`) must NOT invalidate, so the layout
    /// isn't recomputed on every plan-item tick.
    #[test]
    fn apply_todo_snapshot_invalidates_layout_cache_on_presence_transition() {
        use ratatui::layout::Rect;
        use xiaoo_shared::plan::{
            TodoDisplayStatus as SharedTodoStatus, TodoSnapshotItem, TodoSnapshotUpdate,
        };

        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        // Simulate a prior layout pass that cached the terminal area.
        let cached_rect = Rect::new(0, 0, 64, 24);
        state.render_state.cached_area = Some(cached_rect);

        // plan_state starts None; empty update -> no transition -> cache stays.
        runtime.apply_todo_snapshot(
            &mut state,
            TodoSnapshotUpdate {
                title: String::new(),
                items: vec![],
            },
        );
        assert!(state.plan_state.is_none());
        assert_eq!(
            state.render_state.cached_area,
            Some(cached_rect),
            "None->None transition must not invalidate layout cache"
        );

        // None -> Some transition: cache must be invalidated.
        runtime.apply_todo_snapshot(
            &mut state,
            TodoSnapshotUpdate {
                title: "Plan".to_string(),
                items: vec![TodoSnapshotItem {
                    status: SharedTodoStatus::InProgress,
                    content: "step 1".to_string(),
                }],
            },
        );
        assert!(state.plan_state.is_some());
        assert!(
            state.render_state.cached_area.is_none(),
            "None->Some transition must invalidate layout cache (sidebar visibility changed)"
        );

        // Re-cache, then Some -> Some (content only): cache must stay.
        state.render_state.cached_area = Some(cached_rect);
        runtime.apply_todo_snapshot(
            &mut state,
            TodoSnapshotUpdate {
                title: "Plan".to_string(),
                items: vec![
                    TodoSnapshotItem {
                        status: SharedTodoStatus::Completed,
                        content: "step 1".to_string(),
                    },
                    TodoSnapshotItem {
                        status: SharedTodoStatus::InProgress,
                        content: "step 2".to_string(),
                    },
                ],
            },
        );
        assert!(state.plan_state.is_some());
        assert_eq!(
            state.render_state.cached_area,
            Some(cached_rect),
            "Some->Some (content-only) transition must NOT invalidate layout cache"
        );

        // Some -> None transition: cache must be invalidated.
        runtime.apply_todo_snapshot(
            &mut state,
            TodoSnapshotUpdate {
                title: String::new(),
                items: vec![],
            },
        );
        assert!(state.plan_state.is_none());
        assert!(
            state.render_state.cached_area.is_none(),
            "Some->None transition must invalidate layout cache (sidebar visibility changed)"
        );
    }

    #[test]
    fn tool_update_preserves_previous_assistant_message() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(0);
        runtime.set_stream_message_content(&mut state, "before tool", true);

        runtime.apply_tool_update(&mut state, sample_tool_update("call-1"), None);
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
    fn thinking_stream_updates_active_assistant_message() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        runtime.set_stream_message_thinking_content(&mut state, "checking", true);
        runtime.set_stream_message_content(&mut state, "answer", true);

        assert_eq!(state.chat_state.messages.len(), 1);
        assert_eq!(state.chat_state.messages[0].thinking_content, "checking");
        assert_eq!(state.chat_state.messages[0].content, "answer");
        assert!(state.chat_state.messages[0].is_streaming);
    }

    #[test]
    fn stream_updates_preserve_user_scroll_lock() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();
        state.chat_state.stick_to_bottom = false;

        let (tx, rx) = mpsc::unbounded_channel();
        runtime.stream_rx = Some(rx);
        tx.send(SessionTurnUpdate::SetAssistantThinking {
            agent_id: AgentId("cli-agent".to_string()),
            text: "checking".to_string(),
        })
        .expect("thinking update should send");
        tx.send(SessionTurnUpdate::SetAssistantContent {
            agent_id: AgentId("cli-agent".to_string()),
            text: "answer".to_string(),
        })
        .expect("content update should send");

        assert!(runtime.poll_stream_updates(&mut state));

        assert!(!state.chat_state.stick_to_bottom);
        assert_eq!(state.chat_state.messages.len(), 1);
        assert_eq!(state.chat_state.messages[0].thinking_content, "checking");
        assert_eq!(state.chat_state.messages[0].content, "answer");
    }

    #[test]
    fn stream_updates_keep_existing_bottom_stickiness() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();
        state.chat_state.stick_to_bottom = true;

        let (tx, rx) = mpsc::unbounded_channel();
        runtime.stream_rx = Some(rx);
        tx.send(SessionTurnUpdate::SetAssistantContent {
            agent_id: AgentId("cli-agent".to_string()),
            text: "answer".to_string(),
        })
        .expect("content update should send");

        assert!(runtime.poll_stream_updates(&mut state));

        assert!(state.chat_state.stick_to_bottom);
        assert_eq!(state.chat_state.messages[0].content, "answer");
    }

    #[test]
    fn stream_updates_surface_memory_state_transitions() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        let (tx, rx) = mpsc::unbounded_channel();
        runtime.stream_rx = Some(rx);
        tx.send(SessionTurnUpdate::MemoryStatus(MemoryStatus::Disabled))
            .expect("memory status update should send");
        assert!(runtime.poll_stream_updates(&mut state));
        assert_eq!(state.status_panel.memory_status, MemoryStatus::Disabled);

        tx.send(SessionTurnUpdate::MemoryStatus(MemoryStatus::Degraded))
            .expect("memory status update should send");
        assert!(runtime.poll_stream_updates(&mut state));
        assert_eq!(state.status_panel.memory_status, MemoryStatus::Degraded);

        tx.send(SessionTurnUpdate::MemoryStatus(MemoryStatus::Connected))
            .expect("memory status update should send");

        assert!(runtime.poll_stream_updates(&mut state));
        assert_eq!(state.status_panel.memory_status, MemoryStatus::Connected);
    }

    #[test]
    fn background_memory_health_change_updates_status_without_a_turn() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();
        let (health_tx, health_rx) = watch::channel(MemoryAutomationHealth::Healthy);
        *runtime
            .session_gateway
            .memory_health
            .lock()
            .expect("memory health lock should not be poisoned") = Some(health_rx);

        health_tx
            .send(MemoryAutomationHealth::Degraded)
            .expect("memory health receiver should be present");

        assert!(runtime.poll_stream_updates(&mut state));
        assert_eq!(state.status_panel.memory_status, MemoryStatus::Degraded);
    }

    #[test]
    fn child_stream_updates_create_subagent_lane_without_touching_root_messages() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        let (tx, rx) = mpsc::unbounded_channel();
        runtime.stream_rx = Some(rx);
        tx.send(SessionTurnUpdate::SetAssistantContent {
            agent_id: AgentId("child-agent".to_string()),
            text: "child answer".to_string(),
        })
        .expect("content update should send");

        assert!(runtime.poll_stream_updates(&mut state));

        assert!(state.chat_state.messages.is_empty());
        let lane = state
            .chat_state
            .subagent_lanes
            .get("child-agent")
            .expect("child lane should be created");
        assert_eq!(lane.messages.len(), 1);
        assert_eq!(lane.messages[0].content, "child answer");
        assert!(lane.messages[0].is_streaming);
    }

    #[test]
    fn invalid_spawn_output_does_not_create_subagent_lane() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "spawn-1".to_string(),
                tool: "spawn_subagent".to_string(),
                summary: String::new(),
                args_preview: serde_json::json!({
                    "description": "Review code",
                    "task_goal": "Find issues"
                })
                .to_string(),
                command_preview: None,
                command: None,
                detail: "not-json".to_string(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
            Some("main".to_string()),
        );

        assert!(state.chat_state.subagent_lanes.is_empty());
        assert_eq!(state.chat_state.messages.len(), 1);
        assert!(state.chat_state.messages[0].tool_state.is_some());
    }

    #[test]
    fn completed_spawn_output_creates_subagent_lane_with_metadata() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "spawn-2".to_string(),
                tool: "spawn_subagent".to_string(),
                summary: String::new(),
                args_preview: serde_json::json!({
                    "description": "Review code",
                    "task_goal": "Find issues",
                    "task_context": "Focus on tests"
                })
                .to_string(),
                command_preview: None,
                command: None,
                detail: r#"{"agent_id":"child-2"}"#.to_string(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
            Some("main".to_string()),
        );

        let lane = state
            .chat_state
            .subagent_lanes
            .get("child-2")
            .expect("spawn should create lane");
        assert_eq!(lane.parent_agent_id.as_deref(), Some("main"));
        assert_eq!(lane.title, "Review code");
        assert_eq!(lane.description, "Review code");
        assert_eq!(lane.task_goal, "Find issues");
    }

    #[test]
    fn remote_tool_result_does_not_clobber_subagent_spawn_metadata() {
        // Simulates the remote-mode event ordering: the daemon first forwards
        // a `SubagentSpawn` SSE event carrying full metadata, then a `ToolResult`
        // SSE event whose `args_preview` is stripped to an empty string. The
        // `ToolResult` must not overwrite the title/description/task_goal set
        // by `SubagentSpawn` with fallback "Subagent xxx" values.
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        // Step 1: simulate `SubagentSpawn` populating the lane with full
        // metadata (as the daemon-side `SubagentMetaComputingLoopSink` would).
        state.chat_state.ensure_subagent_lane(
            "child-3".to_string(),
            Some("root".to_string()),
            "Refactor module boundaries".to_string(),
            "Split the god module into focused crates".to_string(),
            " Land the split behind a feature flag".to_string(),
        );

        // Step 2: simulate the trailing `ToolResult` event with empty
        // `args_preview` (as `remote.rs` constructs it from the SSE event).
        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "spawn-3".to_string(),
                tool: "spawn_subagent".to_string(),
                summary: String::new(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: r#"{"agent_id":"child-3"}"#.to_string(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
            Some("root".to_string()),
        );

        let lane = state
            .chat_state
            .subagent_lanes
            .get("child-3")
            .expect("lane should still exist after ToolResult");
        assert_eq!(lane.title, "Refactor module boundaries");
        assert_eq!(lane.description, "Split the god module into focused crates");
        assert_eq!(lane.task_goal, " Land the split behind a feature flag");
    }

    #[test]
    fn remote_turn_start_does_not_clobber_subagent_spawn_metadata() {
        // Simulates the remote-mode event ordering: the daemon first forwards
        // a `SubagentSpawn` SSE event carrying full metadata, then the child
        // agent's `TurnStart` arrives. The `TurnStart` handler must NOT call
        // `ensure_subagent_lane` with the generic "Subagent xxx" fallback
        // title (which would clobber the real title via `update_metadata`).
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        // Step 1: simulate `SubagentSpawn` populating the lane with full
        // metadata (as the daemon-side `SubagentMetaComputingLoopSink` would).
        state.chat_state.ensure_subagent_lane(
            "child-3".to_string(),
            Some("root".to_string()),
            "Refactor module boundaries".to_string(),
            "Split the god module into focused crates".to_string(),
            " Land the split behind a feature flag".to_string(),
        );

        // Step 2: feed a `TurnStart` event for the spawned child via the
        // stream channel, then drain it through `poll_stream_updates`.
        let (tx, rx) = mpsc::unbounded_channel::<SessionTurnUpdate>();
        runtime.stream_rx = Some(rx);
        tx.send(SessionTurnUpdate::TurnStart {
            agent_id: AgentId("child-3".to_string()),
            turn: 1,
        })
        .expect("send TurnStart");
        drop(tx);
        let changed = runtime.poll_stream_updates(&mut state);
        assert!(changed, "poll_stream_updates should report a change");

        let lane = state
            .chat_state
            .subagent_lanes
            .get("child-3")
            .expect("lane should still exist after TurnStart");
        assert_eq!(lane.title, "Refactor module boundaries");
        assert_eq!(lane.description, "Split the god module into focused crates");
        assert_eq!(lane.task_goal, " Land the split behind a feature flag");
        assert!(lane.is_running);
        assert_eq!(lane.last_turn, Some(1));
    }

    #[test]
    fn remote_subagent_events_do_not_clobber_subagent_spawn_metadata() {
        // End-to-end verification that the generic "Subagent xxx" fallback
        // title passed by downstream event handlers (TurnStart,
        // SetAssistantContent, Tool) does not clobber the title/description/
        // task_goal populated earlier by a `SubagentSpawn` SSE event. Each
        // handler must use `ensure_subagent_lane_preserve_metadata` (or
        // equivalent) so the lane metadata set by `SubagentSpawn` survives
        // the child agent's entire turn.
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        // Step 1: simulate `SubagentSpawn` populating the lane with full
        // metadata (as the daemon-side `SubagentMetaComputingLoopSink` would).
        state.chat_state.ensure_subagent_lane(
            "child-4".to_string(),
            Some("root".to_string()),
            "Refactor module boundaries".to_string(),
            "Split the god module into focused crates".to_string(),
            " Land the split behind a feature flag".to_string(),
        );

        // Step 2: feed the child agent's turn lifecycle through the stream
        // channel: TurnStart → SetAssistantContent → Tool(Completed).
        let (tx, rx) = mpsc::unbounded_channel::<SessionTurnUpdate>();
        runtime.stream_rx = Some(rx);
        tx.send(SessionTurnUpdate::TurnStart {
            agent_id: AgentId("child-4".to_string()),
            turn: 1,
        })
        .expect("send TurnStart");
        tx.send(SessionTurnUpdate::SetAssistantContent {
            agent_id: AgentId("child-4".to_string()),
            text: "Working on the refactor...".to_string(),
        })
        .expect("send SetAssistantContent");
        tx.send(SessionTurnUpdate::Tool {
            agent_id: AgentId("child-4".to_string()),
            update: ToolExecutionUpdate {
                call_id: "child-4-call-1".to_string(),
                tool: "shell".to_string(),
                summary: "ls".to_string(),
                args_preview: String::new(),
                command_preview: None,
                command: None,
                detail: "src\ntests".to_string(),
                status: ToolExecutionStatus::Completed,
                exit_code: Some(0),
                duration_ms: Some(12),
                file_change: None,
            },
        })
        .expect("send Tool");
        drop(tx);

        // Drain all queued events.
        while runtime.poll_stream_updates(&mut state) {}

        // The metadata populated by `SubagentSpawn` must survive every
        // downstream event in the child's turn.
        let lane = state
            .chat_state
            .subagent_lanes
            .get("child-4")
            .expect("lane should still exist after child turn");
        assert_eq!(lane.title, "Refactor module boundaries");
        assert_eq!(lane.description, "Split the god module into focused crates");
        assert_eq!(lane.task_goal, " Land the split behind a feature flag");
    }

    #[test]
    fn tool_update_drops_empty_streaming_placeholder() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let mut state = test_state();

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(0);

        runtime.apply_tool_update(&mut state, sample_tool_update("call-2"), None);

        assert_eq!(state.chat_state.messages.len(), 1);
        assert!(state.chat_state.messages[0].tool_state.is_some());
        assert!(runtime.stream_message_index.is_none());
    }

    #[test]
    fn tool_update_tracks_session_file_changes_by_call_id() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
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
            None,
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
            None,
        );

        let stats = state
            .session_file_changes()
            .get("src/main.rs")
            .expect("file stats should be tracked");
        assert_eq!(stats.additions, 2);
        assert_eq!(stats.deletions, 1);
    }

    #[test]
    fn completed_file_edit_without_running_update_tracks_args_delta() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");

        let mut state = AppState::new(PathBuf::from("config.toml"), workspace)
            .expect("test app state should initialize");
        state.chat_state.messages.clear();

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "call-args-only".to_string(),
                tool: "file_edit".to_string(),
                summary: String::new(),
                args_preview: serde_json::json!({
                    "file_path": "README.md",
                    "old_string": "[@shen](https://github.com/shen)",
                    "new_string": "[@hypo](https://github.com/hypo)"
                })
                .to_string(),
                command_preview: None,
                command: None,
                detail: String::new(),
                status: ToolExecutionStatus::Completed,
                exit_code: None,
                duration_ms: None,
                file_change: None,
            },
            None,
        );

        let stats = state
            .session_file_changes()
            .get("README.md")
            .expect("file stats should be tracked from args");
        assert_eq!(stats.additions, 1);
        assert_eq!(stats.deletions, 1);
    }

    #[test]
    fn finish_stream_done_clears_per_call_state_but_preserves_file_totals() {
        // Regression for remote-mode memory leak: the SSE stream emits a
        // `Done` event but no `LoopEnd` (the daemon's `SseLoopEventSink`
        // only stores the summary without forwarding it). Without this
        // cleanup in `finish_stream_done`, `tool_file_changes` /
        // `tool_file_baselines` would grow unboundedly across turns. The
        // per-file totals (`session_file_changes`) must survive so the diff
        // panel keeps showing the session's cumulative changes.
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");

        let mut state = AppState::new(PathBuf::from("config.toml"), workspace)
            .expect("test app state should initialize");
        state.chat_state.messages.clear();

        runtime.apply_tool_update(
            &mut state,
            ToolExecutionUpdate {
                call_id: "remote-call-1".to_string(),
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
            None,
        );

        runtime.finish_stream_done(
            &mut state,
            PendingStreamDone {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                estimated_input_tokens: 0,
                messages: Vec::new(),
            },
        );

        // Per-file totals survive `finish_stream_done`.
        let stats = state
            .session_file_changes()
            .get("src/main.rs")
            .expect("file totals should survive finish_stream_done");
        assert_eq!(stats.additions, 2);
        assert_eq!(stats.deletions, 1);
    }

    #[test]
    fn first_token_latency_is_recorded_once_and_completion_uses_reported_prompt_tokens() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
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
        assert_eq!(state.status_panel.input_context_tokens, 10);
        assert!(!state.status_panel.input_context_tokens_estimated);
    }

    #[test]
    fn completion_accumulates_usage_totals_across_turns_without_changing_ctx_semantics() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
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
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                estimated_input_tokens: 18,
                messages: Vec::new(),
            },
        );

        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        runtime.stream_message_index = Some(1);
        runtime.request_start = Some(Instant::now());

        runtime.finish_stream_done(
            &mut state,
            PendingStreamDone {
                prompt_tokens: 25,
                completion_tokens: 7,
                total_tokens: 32,
                estimated_input_tokens: 31,
                messages: Vec::new(),
            },
        );

        assert_eq!(state.status_panel.prompt_tokens, 35);
        assert_eq!(state.status_panel.completion_tokens, 12);
        assert_eq!(state.status_panel.total_tokens, 47);
        assert_eq!(state.status_panel.input_context_tokens, 25);
        assert!(!state.status_panel.input_context_tokens_estimated);
    }

    #[test]
    fn completion_falls_back_to_estimated_input_tokens_when_prompt_usage_is_missing() {
        let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
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
                prompt_tokens: 0,
                completion_tokens: 6,
                total_tokens: 30,
                estimated_input_tokens: 24,
                messages: Vec::new(),
            },
        );

        assert_eq!(state.status_panel.prompt_tokens, 0);
        assert_eq!(state.status_panel.completion_tokens, 6);
        assert_eq!(state.status_panel.total_tokens, 6);
        assert_eq!(state.status_panel.input_context_tokens, 24);
        assert!(state.status_panel.input_context_tokens_estimated);
    }
}
