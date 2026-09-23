use crate::app_state::{AppState, InputMode};
use crate::chat::{Message, TodoSnapshotUpdate, ToolExecutionStatus, ToolExecutionUpdate};
use crate::debug_log;
use crate::session_gateway::SessionTurnUpdate;
use xiaoo_api::chat::AgentId;
use xiaoo_shared::plan::todo_snapshot_from_tool_args;

use super::runtime::{GatewayRuntime, PendingStreamDone, STREAM_REVEAL_CHARS_PER_TICK};

impl GatewayRuntime {
    pub fn poll_stream_updates(&mut self, state: &mut AppState) -> bool {
        #[cfg(debug_assertions)]
        let _start = std::time::Instant::now();
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
                    if self.draining_after_cancel {
                        // The cancelled turn's channel closed without a
                        // terminal update (e.g. the spawned task died).
                        // The user already cancelled; end the drain
                        // quietly instead of injecting the disconnect
                        // notice into the transcript.
                        tracing::debug!("TUI: stream channel closed while draining cancelled turn");
                        self.stream_rx = None;
                        self.draining_after_cancel = false;
                        self.interaction_reply_tx = None;
                        changed = true;
                        break;
                    }
                    self.handle_stream_disconnect(state);
                    changed = true;
                    break;
                }
            };
            changed = true;
            // Draining after Esc: the turn was cancelled, so only the
            // terminal `Done`/`Err` update is consumed (`Done` refreshes
            // `session_messages` with the partial state the backend
            // persisted on cancellation). Every other update — residual
            // TextDeltas, tool events, subagent spawns, prompts — is
            // dropped so the cancelled turn cannot spawn ghost messages
            // or reopen interaction prompts.
            if self.draining_after_cancel
                && !matches!(
                    update,
                    SessionTurnUpdate::Done { .. } | SessionTurnUpdate::Err(_)
                )
            {
                continue;
            }
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
                SessionTurnUpdate::AppendAssistantContent { agent_id, delta } => {
                    if is_root_stream_agent(&agent_id, state) {
                        self.append_stream_message_content(state, &delta);
                        self.record_first_token_latency_if_needed(state);
                    } else {
                        self.append_subagent_stream_message_content(state, &agent_id.0, &delta);
                    }
                }
                SessionTurnUpdate::AppendAssistantThinking { agent_id, delta } => {
                    if is_root_stream_agent(&agent_id, state) {
                        self.append_stream_message_thinking_content(state, &delta);
                        self.record_first_token_latency_if_needed(state);
                    } else {
                        self.append_subagent_stream_message_thinking_content(
                            state,
                            &agent_id.0,
                            &delta,
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
                SessionTurnUpdate::ToolBaseline { call_id, payload } => {
                    state.inject_tool_file_baseline(&call_id, payload);
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
                    cached_tokens: _,
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
                    if self.draining_after_cancel {
                        // Late failure from a turn the user already
                        // cancelled: log it and end the drain without
                        // injecting a ghost error message into the
                        // transcript.
                        tracing::warn!(
                            error = %error,
                            "TUI: cancelled turn ended with an error after Esc"
                        );
                        self.stream_rx = None;
                        self.draining_after_cancel = false;
                        self.interaction_reply_tx = None;
                        continue;
                    }
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

        #[cfg(debug_assertions)]
        if _start.elapsed() > std::time::Duration::from_micros(100) {
            tracing::debug!(target: "perf", elapsed_us = _start.elapsed().as_micros(), changed, "poll_stream_updates");
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
        // Do NOT drop `stream_rx` (nor a pending `Done`): the backend
        // persists the partial loop state only after the in-flight LLM
        // call returns, and then sends the terminal `Done`/`Err` update
        // through this channel. Keep the receiver and enter draining mode
        // so `poll_stream_updates` can consume that terminal update —
        // `Done` refreshes `session_messages` with the persisted partial
        // state, and `/save` / interrupt auto-save wait on it via
        // `settle_in_flight_turn`. Intermediate updates are ignored by the
        // drain guard so no ghost messages appear. Dropping the receiver
        // here is what made a `/save` right after Esc read a store whose
        // `loop_state` was still `None`, losing the whole conversation on
        // `/load`.
        self.draining_after_cancel = self.stream_rx.is_some();
        self.stream_reveal_buffer.clear();
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

    fn append_stream_message_content(&mut self, state: &mut AppState, delta: &str) {
        #[cfg(debug_assertions)]
        let _start = std::time::Instant::now();
        self.ensure_stream_message(state);
        if let Some(message) = self.stream_message_mut(state) {
            message.append_content(delta);
            message.set_streaming(true);
        }
        #[cfg(debug_assertions)]
        tracing::debug!(target: "perf", delta_len = delta.len(), elapsed_us = _start.elapsed().as_micros(), "append_stream_message_content");
    }

    fn append_stream_message_thinking_content(&mut self, state: &mut AppState, delta: &str) {
        #[cfg(debug_assertions)]
        let _start = std::time::Instant::now();
        self.ensure_stream_message(state);
        if let Some(message) = self.stream_message_mut(state) {
            message.thinking_content.push_str(delta);
            message.mark_render_dirty();
            message.set_streaming(true);
        }
        #[cfg(debug_assertions)]
        tracing::debug!(target: "perf", delta_len = delta.len(), elapsed_us = _start.elapsed().as_micros(), "append_stream_message_thinking_content");
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

    fn append_subagent_stream_message_content(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
        delta: &str,
    ) {
        self.ensure_subagent_stream_message(state, agent_id);
        if let Some(message) = self.subagent_stream_message_mut(state, agent_id) {
            message.append_content(delta);
            message.set_streaming(true);
        }
    }

    fn append_subagent_stream_message_thinking_content(
        &mut self,
        state: &mut AppState,
        agent_id: &str,
        delta: &str,
    ) {
        self.ensure_subagent_stream_message(state, agent_id);
        if let Some(message) = self.subagent_stream_message_mut(state, agent_id) {
            message.thinking_content.push_str(delta);
            message.mark_render_dirty();
            message.set_streaming(true);
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
        // A freshly shown or replaced task list (different title or items
        // than the previous one) starts scrolled to the top; status-only
        // updates of the same list keep the user's scroll position (it is
        // clamped against the new content at render time).
        let plan_replaced = !update.items.is_empty()
            && state.plan_state.as_ref().map_or(true, |old| {
                old.title != update.title
                    || old.items.len() != update.items.len()
                    || old
                        .items
                        .iter()
                        .zip(update.items.iter())
                        .any(|((_, old_content), new_item)| old_content != &new_item.content)
            });
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
        if plan_replaced {
            state.plan_panel.reset_scroll();
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
        // The terminal `Done` of a drained (cancelled) turn has been
        // applied; leave draining mode.
        self.draining_after_cancel = false;
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
#[path = "../../../../tests/unit/endside/gateway_api/runtime_stream_test.rs"]
mod tests;
