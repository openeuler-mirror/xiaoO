use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use std::time::{Duration, Instant};

use crate::app::App;
use crate::app_state::{
    current_sandbox_id, sandbox_backend_config, sandbox_display_name, ApiKeyDialogState, InputMode,
    SandboxDialog,
};
use crate::cron_dialog::CronDialogMode;
use crate::gateway::SessionStore;
use crate::input::EventHandler;
use crate::interaction_prompt::{PromptFocus, PromptResolution};
use crate::mcp_service::render_mcp_overview;
use crate::provider_dialog::{DialogFocus, ProviderDialog};
use crate::provider_service::{
    copy_to_clipboard, persist_active_provider_selection, persisted_selection_settings,
    validate_and_connect_api_key,
};
use crate::remote_sessions_service::{
    list_remote_sessions, record_remote_session, RemoteSessionDialog, RemoteSessionDialogEntry,
    RemoteSessionDialogMode, RemoteSessionRecord,
};
use crate::services::input_history::save_input_history;
use crate::services::turn_delete::DeleteDialog;
use crate::session_snapshot_service::{
    apply_snapshot, list_session_snapshots, load_snapshot, load_snapshot_by_key,
    manual_snapshot_name_from_command, save_manual_snapshot, snapshot_name_from_command,
    SessionSnapshotDialog, SessionSnapshotListEntry,
};
use crate::skills_service::render_skills_overview;
use crate::workspace_service::{first_token_is_dir_command, resolve_dir_command};

/// Window after an ESC event during which stray `Char` events are dropped.
/// See `App::esc_discard_until`. Split mouse/scroll sequence remnants
/// arrive within ~1ms of the ESC event; human key presses are >50ms apart.
const ESC_REMNANT_DISCARD_WINDOW: Duration = Duration::from_millis(5);

impl App {
    pub(crate) async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return Ok(());
        }

        // Opening the discard window on ANY Esc event (including one that
        // was itself parsed from a split mouse/scroll sequence) is correct:
        // if the user pressed a real ESC key, a stale sequence remnant can
        // follow within ~1ms; if the "Esc" was the swallowed head of a split
        // mouse sequence, the remnant follows for the same reason.
        if key.code == KeyCode::Esc {
            self.esc_discard_until = Some(Instant::now() + ESC_REMNANT_DISCARD_WINDOW);
        }

        // Copy/quit key split:
        //   Ctrl+C        → quit unconditionally (SIGINT semantics).
        //   Ctrl+Shift+C  → copy the active selection (input or transcript);
        //                   no-op when nothing is selected. Note: most
        //                   terminals (mate-terminal, alacritty, xterm,
        //                   gnome-terminal…) intercept Ctrl+Shift+C for
        //                   their own screen-selection copy, so the reliable
        //                   in-app copy shortcut is Ctrl+Insert (or the
        //                   mouse drag-select).
        //   Ctrl+Insert   → copy the active selection (terminal-independent
        //                   X11 copy key, unbound by default in common
        //                   terminals).
        // Protocol caveat: Ctrl+C and Ctrl+Shift+C are indistinguishable at
        // the protocol level — both send byte 0x03, which crossterm
        // normalises to Char('c') + CONTROL with the SHIFT modifier lost
        // (crossterm-0.28.1/src/event/sys/unix/parse.rs:106). With a
        // selection active the intent is most likely a copy (Ctrl+Shift+C
        // collapsed), so copy; without one it quits (Ctrl+C). This applies
        // to BOTH the raw \x03 branch and the Char('c')+CONTROL branch
        // below, so behaviour does not differ by terminal class.
        if key.code == KeyCode::Char('\x03') {
            if self.copy_active_selection() {
                return Ok(());
            }
            self.state.should_quit = true;
            self.state.quit_via_interrupt = true;
            return Ok(());
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
            if self.copy_active_selection() {
                return Ok(());
            }
            self.state.should_quit = true;
            self.state.quit_via_interrupt = true;
            return Ok(());
        }
        // Ctrl+Insert / Ctrl+Shift+Insert: copy the active selection.
        // Terminal-independent X11 copy keys; Ctrl+Shift+C is intercepted
        // by most terminals, and some bind plain Ctrl+Insert as well, so
        // both variants are accepted.
        if key.code == KeyCode::Insert
            && key
                .modifiers
                .intersects(event::KeyModifiers::CONTROL)
        {
            self.copy_active_selection();
            return Ok(());
        }

        // Ctrl+X: cut selected input text.
        if key.code == KeyCode::Char('x') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
            if let Some(text) = self.state.chat_state.input.delete_selected() {
                if let Err(e) = copy_to_clipboard(&text) {
                    tracing::warn!("copy_to_clipboard failed: {}", e);
                    self.state.set_copy_error_notice();
                } else {
                    self.state.set_copy_notice();
                }
                self.state.chat_state.reset_input_history_navigation();
                self.state.note_input_changed();
            }
            return Ok(());
        }

        if is_leave_subagent_view_key(&key)
            && self.state.is_subagent_view_active()
            && self.state.api_key_dialog.is_none()
            && self.state.provider_dialog.is_none()
            && self.state.sandbox_dialog.is_none()
            && self.state.remote_session_dialog.is_none()
            && self.state.session_snapshot_dialog.is_none()
            && self.state.delete_dialog.is_none()
            && self.state.interaction_prompt.is_none()
        {
            self.state.leave_subagent_view();
            return Ok(());
        }

        if key.code == KeyCode::Esc && self.state.chat_state.is_loading {
            self.gateway.cancel_streaming(&mut self.state);
            return Ok(());
        }

        if self.state.api_key_dialog.is_some() {
            return self.handle_api_key_dialog_key(key);
        }

        if self.state.input_mode == InputMode::InteractionPrompt {
            return self.handle_interaction_prompt_key(key);
        }

        if self.state.input_mode == InputMode::SessionSnapshotSelection {
            return self.handle_session_snapshot_selection_key(key).await;
        }

        if self.state.input_mode == InputMode::SandboxSelection {
            return self.handle_sandbox_selection_key(key);
        }

        if self.state.input_mode == InputMode::RemoteSessionSelection {
            return self.handle_remote_session_selection_key(key).await;
        }

        if self.state.input_mode == InputMode::TurnDelete {
            return self.handle_turn_delete_key(key).await;
        }

        match self.state.input_mode {
            InputMode::Editing => self.handle_editing_mode_key(key).await,
            InputMode::ProviderSelection => self.handle_provider_selection_key(key),
            InputMode::SandboxSelection => Ok(()),
            InputMode::RemoteSessionSelection => Ok(()),
            InputMode::SessionSnapshotSelection => Ok(()),
            InputMode::InteractionPrompt => Ok(()),
            InputMode::TurnDelete => Ok(()),
            InputMode::CronManagement => self.handle_cron_management_key(key).await,
        }
    }

    /// Copy the active selection (input box first, then transcript) to the
    /// clipboard. Returns `true` when a selection was copied. The selection
    /// is cleared only on success; the success/error toast is shown either
    /// way so the user always gets feedback.
    pub(crate) fn copy_active_selection(&mut self) -> bool {
        if let Some(text) = self
            .state
            .chat_state
            .input
            .selected_text()
            .map(str::to_owned)
        {
            match copy_to_clipboard(&text) {
                Ok(()) => {
                    self.state.chat_state.input.clear_selection();
                    self.state.set_copy_notice();
                }
                Err(e) => {
                    tracing::warn!("copy_to_clipboard failed: {}", e);
                    self.state.set_copy_error_notice();
                }
            }
            return true;
        }
        if let Some(text) = self.state.transcript_selected_text() {
            match copy_to_clipboard(&text) {
                Ok(()) => {
                    self.state.transcript_selection = None;
                    self.state.set_copy_notice();
                }
                Err(e) => {
                    tracing::warn!("copy_to_clipboard failed: {}", e);
                    self.state.set_copy_error_notice();
                }
            }
            return true;
        }
        false
    }

    fn handle_api_key_dialog_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(mut dialog) = self.state.api_key_dialog.take() else {
            tracing::warn!("TUI: api key dialog state missing while handling key event");
            self.state.input_mode = InputMode::Editing;
            return Ok(());
        };
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Editing;
            }
            KeyCode::Enter => {
                let api_key = dialog.input.value().trim().to_string();
                if api_key.is_empty() {
                    dialog.error = Some("API key cannot be empty.".to_string());
                    self.state.api_key_dialog = Some(dialog);
                } else {
                    let provider = dialog.provider.clone();
                    let model = dialog.model.clone();
                    match validate_and_connect_api_key(&mut self.state, provider, model, &api_key) {
                        Ok(()) => {}
                        Err(error) => {
                            dialog.error = Some(error);
                            self.state.api_key_dialog = Some(dialog);
                        }
                    }
                }
            }
            _ => {
                dialog.input.handle_event(&Event::Key(key));
                self.state.api_key_dialog = Some(dialog);
            }
        }
        Ok(())
    }

    fn handle_interaction_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        let mut resolution = None;

        if let Some(prompt) = self.state.interaction_prompt.as_mut() {
            match key.code {
                KeyCode::Esc => {
                    resolution = Some(PromptResolution::Cancelled);
                }
                KeyCode::Tab => {
                    prompt.toggle_focus();
                }
                KeyCode::Enter => {
                    if prompt.request.multi_select {
                        let choice_ids: Vec<String> = prompt
                            .multi_checked
                            .iter()
                            .enumerate()
                            .filter(|(_, checked)| **checked)
                            .map(|(index, _)| prompt.request.choices[index].id.clone())
                            .collect();
                        resolution = Some(PromptResolution::Multi { choice_ids });
                    } else {
                        let choice_id = prompt
                            .request
                            .choices
                            .get(prompt.selected)
                            .map(|choice| choice.id.clone())
                            .unwrap_or_default();
                        let supplement = if prompt.request.allow_custom_input {
                            let value = prompt.supplement.value().trim();
                            if value.is_empty() {
                                None
                            } else {
                                Some(value.to_string())
                            }
                        } else {
                            None
                        };
                        resolution = Some(PromptResolution::Single {
                            choice_id,
                            supplement,
                        });
                    }
                }
                KeyCode::Char(' ') => {
                    if prompt.focus == PromptFocus::List {
                        prompt.toggle_multi_at_cursor();
                    } else {
                        prompt.supplement.handle_event(&Event::Key(key));
                    }
                }
                KeyCode::Up => {
                    if prompt.focus == PromptFocus::List {
                        prompt.move_up();
                    } else {
                        prompt.supplement.handle_event(&Event::Key(key));
                    }
                }
                KeyCode::Down => {
                    if prompt.focus == PromptFocus::List {
                        prompt.move_down();
                    } else {
                        prompt.supplement.handle_event(&Event::Key(key));
                    }
                }
                KeyCode::PageUp => {
                    if prompt.focus == PromptFocus::List {
                        prompt.page_up();
                    } else {
                        prompt.supplement.handle_event(&Event::Key(key));
                    }
                }
                KeyCode::PageDown => {
                    if prompt.focus == PromptFocus::List {
                        prompt.page_down();
                    } else {
                        prompt.supplement.handle_event(&Event::Key(key));
                    }
                }
                _ => {
                    if prompt.focus == PromptFocus::Supplement {
                        prompt.supplement.handle_event(&Event::Key(key));
                    } else if prompt.request.allow_custom_input {
                        match key.code {
                            KeyCode::Char(_) => {
                                let modifiers = key.modifiers;
                                if modifiers.is_empty() || modifiers == event::KeyModifiers::SHIFT {
                                    prompt.focus = PromptFocus::Supplement;
                                    prompt.supplement.handle_event(&Event::Key(key));
                                }
                            }
                            KeyCode::Backspace
                            | KeyCode::Delete
                            | KeyCode::Left
                            | KeyCode::Right
                            | KeyCode::Home
                            | KeyCode::End => {
                                prompt.focus = PromptFocus::Supplement;
                                prompt.supplement.handle_event(&Event::Key(key));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if let Some(resolution) = resolution {
            self.gateway
                .resolve_interaction_prompt(&mut self.state, resolution);
        }
        Ok(())
    }

    async fn handle_editing_mode_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.state.is_subagent_view_active() {
            match key.code {
                KeyCode::Esc => {
                    self.state.transcript_selection = None;
                }
                KeyCode::Up if key.modifiers.is_empty() => {
                    self.state.active_transcript_scroll_up();
                }
                KeyCode::Down if key.modifiers.is_empty() => {
                    self.state.active_transcript_scroll_down();
                }
                KeyCode::PageUp => {
                    for _ in 0..10 {
                        self.state.active_transcript_scroll_up();
                    }
                }
                KeyCode::PageDown => {
                    for _ in 0..10 {
                        self.state.active_transcript_scroll_down();
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        if key.code == KeyCode::Tab {
            let has_slash_prefix = crate::slash_complete::slash_typed_prefix(
                self.state.chat_state.input.value(),
                self.state.chat_state.input.cursor(),
            )
            .is_some();
            if has_slash_prefix {
                if self.state.slash_menu_visible() {
                    self.state.apply_slash_selection();
                } else {
                    crate::slash_complete::apply_slash_tab(
                        &mut self.state.chat_state.input,
                        &self.state.external_commands,
                    );
                }
                self.state.chat_state.reset_input_history_navigation();
                self.state.note_input_changed();
            } else {
                self.state.cycle_agent_role(false);
            }
            return Ok(());
        }

        if key.code == KeyCode::Char('t') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
            self.state.cycle_reasoning_effort();
            return Ok(());
        }

        if self.state.slash_menu_visible() {
            match key.code {
                KeyCode::Up => {
                    self.state.slash.selected = self.state.slash.selected.saturating_sub(1);
                    return Ok(());
                }
                KeyCode::Down => {
                    let candidate_count = self.state.slash_candidate_count();
                    if candidate_count > 0 {
                        self.state.slash.selected =
                            (self.state.slash.selected + 1).min(candidate_count - 1);
                    }
                    return Ok(());
                }
                KeyCode::Enter => {
                    self.state.apply_slash_selection();
                    self.state.dismiss_current_slash_menu();
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.state.dismiss_current_slash_menu();
                    return Ok(());
                }
                KeyCode::PageUp | KeyCode::PageDown => {
                    return Ok(());
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => {
                // Esc clears an active transcript selection (mirrors opencode's Esc handler).
                self.state.transcript_selection = None;
            }
            KeyCode::Enter => {
                if editing_key_inserts_newline(key.code, key.modifiers) {
                    self.insert_newline_into_input();
                } else {
                    self.submit_editing_input().await?
                }
            }
            // Ctrl+J also inserts a newline. See `editing_key_inserts_newline`
            // for why both the Enter and Char('j') branches route here.
            KeyCode::Char('j') | KeyCode::Char('J')
                if editing_key_inserts_newline(key.code, key.modifiers) =>
            {
                self.insert_newline_into_input();
            }
            KeyCode::Up if key.modifiers.is_empty() => {
                if self.state.chat_state.input_history_cursor.is_some()
                    || self.state.chat_state.input.value().is_empty()
                {
                    self.state.chat_state.previous_input_history();
                    self.state.note_input_changed();
                } else {
                    self.state.chat_state.input.handle_event(&Event::Key(key));
                    self.state.note_input_changed();
                }
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                if self.state.chat_state.input_history_cursor.is_some()
                    || self.state.chat_state.input.value().is_empty()
                {
                    self.state.chat_state.next_input_history();
                    self.state.note_input_changed();
                } else {
                    self.state.chat_state.input.handle_event(&Event::Key(key));
                    self.state.note_input_changed();
                }
            }
            KeyCode::PageUp => {
                self.state.chat_state.scroll_page_up();
            }
            KeyCode::PageDown => {
                self.state.chat_state.scroll_page_down();
            }
            _ => {
                // Drop characters that are clearly not deliberate typing:
                //
                // 1. ESC-remnant window: a split mouse/scroll sequence
                //    (`\x1b[<b;x;yM`) delivers its leftover `[<b;x;yM` as
                //    `Char` events back-to-back with the ESC event that
                //    swallowed the sequence head. Within the window these
                //    are remnants, not keystrokes.
                // 2. ALT-modified chars: an ESC followed by a regular key
                //    byte in the same read batch is parsed as Alt+char (or
                //    arrives as such from the terminal); the char part must
                //    not be typed into the input box. Pure ALT only — the
                //    Input layer handles Ctrl combos (Ctrl+A select-all,
                //    unknown Ctrl combos ignored), and AltGr (CONTROL|ALT)
                //    must stay a valid input on European layouts.
                if self.esc_discard_until.is_some_and(|until| Instant::now() < until) {
                    if let KeyCode::Char(_) = key.code {
                        return Ok(());
                    }
                }
                if let KeyCode::Char(_) = key.code {
                    if key.modifiers.contains(event::KeyModifiers::ALT)
                        && !key.modifiers.contains(event::KeyModifiers::CONTROL)
                    {
                        return Ok(());
                    }
                }
                let before = self.state.chat_state.input.value().to_string();
                self.state.chat_state.input.handle_event(&Event::Key(key));
                if self.state.chat_state.input.value() != before {
                    self.state.chat_state.reset_input_history_navigation();
                }
                self.state.note_input_changed();
            }
        }
        Ok(())
    }

    fn insert_newline_into_input(&mut self) {
        self.state
            .chat_state
            .input
            .handle(crate::input::InputRequest::InsertChar('\n'));
        self.state.chat_state.reset_input_history_navigation();
        self.state.note_input_changed();
    }

    async fn submit_editing_input(&mut self) -> Result<()> {
        let user_input = self.state.chat_state.input.value().to_string();
        if user_input.trim().is_empty() {
            return Ok(());
        }

        let trimmed = user_input.trim();
        self.record_submitted_input_history(&user_input).await;

        if trimmed.eq_ignore_ascii_case("/delete") {
            self.state.chat_state.input.reset();
            if self.state.chat_state.is_loading {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(
                        "当前任务仍在运行。请等待它结束，或先按 Esc 取消。".to_string(),
                    ));
            } else if let Some(dialog) = DeleteDialog::new(&self.state.chat_state.messages) {
                self.state.input_mode = InputMode::TurnDelete;
                self.state.delete_dialog = Some(dialog);
            } else {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(
                        "当前会话没有可删除的对话。".to_string(),
                    ));
            }
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/new") {
            let old_session_id = self.state.session_id.clone();
            let was_remote_mode = self.gateway.is_remote_mode();

            self.gateway.reset_for_new_session(&mut self.state);
            self.state.reset_for_new_session();

            if was_remote_mode {
                tracing::info!(old_session_id = %old_session_id, "Detaching old remote session (preserving on daemon)");
                self.gateway
                    .detach_remote_session_bounded(&old_session_id, "new_session")
                    .await;
            } else {
                tracing::info!(old_session_id = %old_session_id, "Releasing old local session backend");
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    self.gateway.release_session_backend(&old_session_id),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "Failed to release local session backend");
                    }
                    Err(_) => {
                        tracing::warn!("Local session backend release timed out after 5 seconds");
                    }
                }
            }

            if let Some(base_url) = self.gateway.remote_base_url() {
                self.state
                    .status_panel
                    .set_backend(format!("Remote: {}", base_url));
                self.state.status_panel.set_remote_workspace(base_url);
            }
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system("New session started"));
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if self.state.chat_state.is_loading {
            if let Some((body, command_context)) = self.external_command_body(trimmed) {
                // Slash commands carry `command_context` and must start their
                // own turn so the `*.Chat.command.before` hook fires with
                // `{ command, arguments }`. Enqueuing the body to
                // `pending_user_messages` as well would let
                // `drain_pending_user_messages` inject it into the *current*
                // turn's history as a plain user message — only
                // `*.Chat.message.received` runs there, never
                // `command.before` — and then `start_next_queued_turn` would
                // write it again (with the command hook) when starting the
                // deferred turn. The body would appear twice and the command
                // hook would fire for only the second copy. Queue it solely
                // as a pending turn so it is processed once, after the
                // running turn ends.
                self.state
                    .chat_state
                    .enqueue_pending_turn(body, Some(command_context), 0);
                return Ok(());
            }

            if trimmed.starts_with('/') || first_token_is_dir_command(trimmed) {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(
                        "当前任务仍在运行。普通消息会进入待发送队列；控制命令请等待当前任务结束，或先按 Esc 取消。"
                            .to_string(),
                    ));
            } else {
                // Queue free-form text solely as a pending turn. Mirrors the
                // slash-command branch above: enqueuing to
                // `pending_user_messages` as well would let
                // `drain_pending_user_messages` inject a copy into the
                // *current* turn's history (where `*.Chat.message.received`
                // fires), and then `start_next_queued_turn` would write it
                // again as the next turn's user message (where the same hook
                // fires a second time). The message would appear twice and
                // any `Transform` would be applied twice (e.g. a prefix
                // added twice). Queue it once so it is processed once, after
                // the running turn ends.
                self.state
                    .chat_state
                    .enqueue_pending_turn(user_input, None, 0);
            }
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if is_named_slash_command(trimmed, "/save") {
            self.state.chat_state.input.reset();
            match manual_snapshot_name_from_command(trimmed, "/save") {
                Ok(requested_name) => {
                    let record = self.gateway.session_snapshot(&self.state.session_id).await;
                    match save_manual_snapshot(&self.state, record, requested_name.as_deref()) {
                        Ok((path, context)) => {
                            let name = context.name.clone();
                            self.state.current_snapshot_context = Some(context);
                            self.state
                                .chat_state
                                .messages
                                .push(crate::chat::Message::system(format!(
                                    "Session snapshot saved: {} ({})",
                                    name,
                                    path.display()
                                )))
                        }
                        Err(error) => {
                            self.state
                                .chat_state
                                .messages
                                .push(crate::chat::Message::error(format!(
                                    "Save snapshot failed: {error:#}"
                                )))
                        }
                    }
                }
                Err(error) => self
                    .state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::error(format!(
                        "Save snapshot failed: {error:#}"
                    ))),
            }
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if is_named_slash_command(trimmed, "/load") {
            self.state.chat_state.input.reset();
            if slash_command_argument(trimmed, "/load").is_none() {
                self.open_load_snapshot_dialog();
            } else {
                match snapshot_name_from_command(trimmed, "/load").and_then(|name| {
                    let matches = load_snapshot(&name)?;
                    Ok((name, matches))
                }) {
                    Ok((name, matches)) => {
                        if matches.len() == 1 {
                            let (snapshot_key, snapshot, parent_chain) =
                                matches.into_iter().next().unwrap();
                            self.load_snapshot_into_state(
                                &snapshot_key,
                                &name,
                                snapshot,
                                parent_chain,
                            )
                            .await;
                        } else {
                            let entries: Vec<SessionSnapshotListEntry> = matches
                                .into_iter()
                                .map(|(snapshot_key, snapshot, parent_chain)| {
                                    SessionSnapshotListEntry {
                                        kind: snapshot.kind,
                                        name: snapshot.name,
                                        snapshot_key,
                                        saved_at_ms: snapshot.saved_at_ms,
                                        parent_name: parent_chain.last().cloned(),
                                        parent_chain,
                                        depth: 0,
                                        base_manual_name: None,
                                    }
                                })
                                .collect();
                            self.state.session_snapshot_dialog =
                                Some(SessionSnapshotDialog::manual_only(entries));
                            self.state.input_mode = InputMode::SessionSnapshotSelection;
                            self.state
                                .chat_state
                                .messages
                                .push(crate::chat::Message::system(format!(
                                    "Multiple snapshots named '{}' found. Please select one.",
                                    name
                                )));
                        }
                    }
                    Err(error) => self
                        .state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::error(format!(
                            "Load snapshot failed: {error:#}"
                        ))),
                }
            }
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/connect") {
            self.state.chat_state.input.reset();
            self.open_provider_selection_dialog();
            return Ok(());
        }

        if is_named_slash_command(trimmed, "/remote") {
            self.handle_remote_command(trimmed).await;
            return Ok(());
        }

        if is_named_slash_command(trimmed, "/sessions") {
            self.handle_sessions_command(trimmed).await;
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/sandbox") {
            self.state.chat_state.input.reset();
            self.open_sandbox_selection_dialog();
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/skills") {
            self.state.chat_state.input.reset();
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(render_skills_overview(
                    &self.state.agent_config,
                )));
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/mcp") {
            self.state.chat_state.input.reset();
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(render_mcp_overview(
                    &self.state.agent_config,
                )));
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/cron") {
            self.state.chat_state.input.reset();
            self.open_cron_dialog();
            return Ok(());
        }
        if first_token_is_dir_command(trimmed) {
            match resolve_dir_command(trimmed, &self.state.workspace) {
                Ok(path) => {
                    self.state.workspace = path;
                    self.state.status_panel.set_workspace(&self.state.workspace);
                    self.state.sync_diff_tracker_workspace();
                    self.state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::system(format!(
                            "Workspace: {}",
                            self.state.workspace.display()
                        )));
                    self.state.chat_state.stick_to_bottom = true;
                    self.state.chat_state.input.reset();
                }
                Err(error) => {
                    self.state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::system(error));
                    self.state.chat_state.stick_to_bottom = true;
                    self.state.chat_state.input.reset();
                }
            }
            return Ok(());
        }

        // External commands from ~/.xiaoo/commands/
        if let Some((body, command_context)) = self.external_command_body(trimmed) {
            self.state.chat_state.input.reset();
            if let Err(error) = self
                .gateway
                .start_turn_for_command(&mut self.state, body, command_context)
                .await
            {
                let display_error = crate::error_log::record_tui_error("start_turn", &error);
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::error(display_error));
                self.state.chat_state.stick_to_bottom = true;
            }
            return Ok(());
        }

        if let Err(error) = self.gateway.start_turn(&mut self.state, user_input).await {
            let display_error = crate::error_log::record_tui_error("start_turn", &error);
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::error(display_error));
            self.state.chat_state.stick_to_bottom = true;
        }
        Ok(())
    }

    async fn record_submitted_input_history(&mut self, input: &str) {
        self.state.chat_state.record_input_history(input);
        let history = self.state.chat_state.input_history.clone();

        match tokio::task::spawn_blocking(move || save_input_history(&history)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!("failed to persist input history: {error:#}");
            }
            Err(error) => {
                tracing::warn!("input history persistence task failed: {error}");
            }
        }
    }

    fn external_command_body(
        &self,
        trimmed: &str,
    ) -> Option<(String, agent_types::chat::CommandContext)> {
        expand_external_command(trimmed, &self.state.external_commands)
    }

    async fn handle_remote_command(&mut self, trimmed: &str) {
        self.state.chat_state.input.reset();
        let arg = slash_command_argument(trimmed, "/remote")
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let Some(arg) = arg else {
            self.open_remote_session_dialog();
            return;
        };

        let message = match arg {
            "status" => crate::chat::Message::system(self.gateway.remote_status(&self.state).await),
            "off" => match self.gateway.disconnect_remote(&mut self.state).await {
                Ok(()) => crate::chat::Message::system(format!(
                    "Remote disconnected. Remote sessions are kept on the daemon. Backend: {}.",
                    sandbox_display_name(&self.state.agent_config.operation_backend)
                )),
                Err(error) => {
                    crate::chat::Message::error(format!("Remote disconnect failed: {error}"))
                }
            },
            "close" => {
                let session_id = self.state.session_id.clone();
                match self.gateway.close_remote_session(&session_id).await {
                    Ok(()) => crate::chat::Message::system(format!(
                        "Remote session closed on daemon: {session_id}"
                    )),
                    Err(error) => {
                        crate::chat::Message::error(format!("Remote session close failed: {error}"))
                    }
                }
            }
            base_url => {
                let base_url = normalize_remote_url_input(base_url);
                let token_env = self
                    .state
                    .agent_config
                    .tui
                    .remote
                    .as_ref()
                    .and_then(|remote| remote.bearer_token_env.clone());
                match self
                    .gateway
                    .connect_remote(&mut self.state, base_url.clone(), token_env)
                    .await
                {
                    Ok(message) => {
                        let _ = record_remote_session(
                            &self.state.session_id,
                            &base_url,
                            self.state
                                .agent_config
                                .tui
                                .remote
                                .as_ref()
                                .and_then(|remote| remote.bearer_token_env.clone()),
                            None,
                        );
                        crate::chat::Message::system(message)
                    }
                    Err(error) => {
                        crate::chat::Message::error(format!("Remote connect failed: {error}"))
                    }
                }
            }
        };
        self.state.chat_state.messages.push(message);
        self.state.chat_state.stick_to_bottom = true;
    }

    fn open_remote_session_dialog(&mut self) {
        match list_remote_sessions() {
            Ok(records) => {
                self.state.input_mode = InputMode::RemoteSessionSelection;
                self.state.remote_session_dialog = Some(RemoteSessionDialog::new(records));
            }
            Err(error) => {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::error(format!(
                        "Remote session history failed: {error:#}"
                    )));
                self.state.chat_state.stick_to_bottom = true;
            }
        }
    }

    /// `/sessions [session_id]` — switch the TUI focus to another session on
    /// the currently connected daemon. With no argument, opens a dialog
    /// listing every known session on that daemon (filtered out of the local
    /// `remote_sessions.json` registry) and marks the active one. With an
    /// explicit `session_id`, switches immediately. The daemon-side session
    /// is assumed to already exist (created via a hook or `/remote`); the
    /// TUI only resets local state and re-points at it.
    async fn handle_sessions_command(&mut self, trimmed: &str) {
        self.state.chat_state.input.reset();

        let Some(base_url) = self.gateway.remote_base_url().map(str::to_string) else {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(
                    "No remote daemon connected. Use /remote <url> first.".to_string(),
                ));
            self.state.chat_state.stick_to_bottom = true;
            return;
        };
        let normalized = crate::remote_sessions_service::normalize_base_url(&base_url);

        if let Some(arg) = slash_command_argument(trimmed, "/sessions")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if arg == self.state.session_id {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(format!(
                        "Already on session: {arg}"
                    )));
            } else {
                // Validate the target session is recorded for the currently
                // connected daemon *before* entering `switch_to_remote_session`,
                // which unconditionally clears the local transcript. Otherwise
                // a mistyped/non-existent session id would leave the user with
                // an empty conversation and no way to recover the prior one.
                let known = match list_remote_sessions() {
                    Ok(records) => records.iter().any(|record| {
                        record.session_id == arg
                            && crate::remote_sessions_service::normalize_base_url(&record.base_url)
                                == normalized
                    }),
                    Err(_) => false,
                };
                if !known {
                    self.state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::error(format!(
                            "Session {arg} is not recorded for {normalized}. \
                             Run /sessions without an argument to pick a known session."
                        )));
                } else {
                    self.switch_to_remote_session(arg.to_string()).await;
                }
            }
            self.state.chat_state.stick_to_bottom = true;
            return;
        }

        let records = match list_remote_sessions() {
            Ok(records) => records,
            Err(error) => {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::error(format!(
                        "Remote session history failed: {error:#}"
                    )));
                self.state.chat_state.stick_to_bottom = true;
                return;
            }
        };
        let filtered: Vec<RemoteSessionRecord> = records
            .into_iter()
            .filter(|record| {
                crate::remote_sessions_service::normalize_base_url(&record.base_url) == normalized
            })
            .collect();

        if filtered.is_empty() {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(format!(
                    "No other sessions recorded for {normalized}. Use /remote to create one."
                )));
            self.state.chat_state.stick_to_bottom = true;
            return;
        }

        if filtered.len() == 1
            && filtered
                .iter()
                .any(|record| record.session_id == self.state.session_id)
        {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(format!(
                    "Only the current session is recorded for {normalized}."
                )));
            self.state.chat_state.stick_to_bottom = true;
            return;
        }

        self.state.input_mode = InputMode::RemoteSessionSelection;
        self.state.remote_session_dialog = Some(RemoteSessionDialog::new_for_switch(
            filtered,
            Some(self.state.session_id.clone()),
        ));
    }

    async fn handle_remote_session_selection_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(mode) = self
            .state
            .remote_session_dialog
            .as_ref()
            .map(|dialog| dialog.mode)
        else {
            self.state.input_mode = InputMode::Editing;
            return Ok(());
        };

        match mode {
            RemoteSessionDialogMode::List => self.handle_remote_session_list_key(key).await,
            RemoteSessionDialogMode::NewUrl => self.handle_remote_session_new_url_key(key).await,
        }
    }

    async fn handle_remote_session_list_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Editing;
                self.state.remote_session_dialog = None;
            }
            KeyCode::Up => {
                if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                    dialog.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                    dialog.move_down();
                }
            }
            KeyCode::Enter => {
                let selected = self
                    .state
                    .remote_session_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.selected_entry().cloned());
                match selected {
                    Some(RemoteSessionDialogEntry::Existing(record)) => {
                        self.state.input_mode = InputMode::Editing;
                        self.state.remote_session_dialog = None;
                        self.activate_remote_session(record).await;
                    }
                    Some(RemoteSessionDialogEntry::New) => {
                        if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                            dialog.enter_new_url_mode();
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_remote_session_new_url_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                    dialog.mode = RemoteSessionDialogMode::List;
                    dialog.error = None;
                }
            }
            KeyCode::Enter => {
                let url = self
                    .state
                    .remote_session_dialog
                    .as_ref()
                    .map(|dialog| dialog.url_input.value().trim().to_string())
                    .unwrap_or_default();
                if url.is_empty() {
                    if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                        dialog.error =
                            Some("请输入 daemon URL，例如 http://127.0.0.1:8070".to_string());
                    }
                    return Ok(());
                }
                self.create_new_remote_session(normalize_remote_url_input(&url))
                    .await;
            }
            _ => {
                if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                    dialog.url_input.handle_event(&Event::Key(key));
                    dialog.error = None;
                }
            }
        }
        Ok(())
    }

    async fn activate_remote_session(&mut self, record: RemoteSessionRecord) {
        if record.session_id == self.state.session_id {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(format!(
                    "Already on session: {}",
                    record.session_id
                )));
            self.state.chat_state.stick_to_bottom = true;
            return;
        }
        let system_message = format!(
            "Remote session selected: {} ({})",
            record.session_id, record.base_url
        );
        self.apply_remote_session_switch(
            record.session_id.clone(),
            record.base_url.clone(),
            record.bearer_token_env.clone(),
            system_message,
        )
        .await;
    }

    async fn create_new_remote_session(&mut self, base_url: String) {
        let token_env = self
            .state
            .agent_config
            .tui
            .remote
            .as_ref()
            .and_then(|remote| remote.bearer_token_env.clone());
        match self
            .gateway
            .connect_remote(&mut self.state, base_url.clone(), token_env.clone())
            .await
        {
            Ok(message) => {
                self.gateway.reset_for_new_session(&mut self.state);
                self.state.reset_for_new_session();
                self.gateway
                    .configure_remote(&mut self.state, base_url.clone(), token_env.clone());
                let session_id = self.state.session_id.clone();
                let _ = record_remote_session(&session_id, &base_url, token_env, None);
                self.state.input_mode = InputMode::Editing;
                self.state.remote_session_dialog = None;
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(format!(
                        "{message}\nNew remote session: {session_id}"
                    )));
                self.state.chat_state.stick_to_bottom = true;
            }
            Err(error) => {
                if let Some(dialog) = self.state.remote_session_dialog.as_mut() {
                    dialog.error = Some(format!("Remote connect failed: {error}"));
                }
            }
        }
    }

    fn handle_provider_selection_key(&mut self, key: KeyEvent) -> Result<()> {
        let mut selection_to_apply = None;
        let mut need_api_key_dialog = None;
        let mut close_dialog = false;
        let mut check_local_fetch = false;

        if let Some(dialog) = self.state.provider_dialog.as_mut() {
            match key.code {
                KeyCode::Esc => {
                    close_dialog = true;
                }
                KeyCode::Enter => {
                    if let Some((provider_name, model_id)) = dialog.selected() {
                        let (api_key_env, api_base) =
                            persisted_selection_settings(&self.state.agent_config, &provider_name);
                        if api_key_env.is_some()
                            && api_key_env
                                .as_deref()
                                .and_then(|name| std::env::var(name).ok())
                                .filter(|value| !value.trim().is_empty())
                                .is_none()
                        {
                            need_api_key_dialog = Some(ApiKeyDialogState {
                                provider: provider_name,
                                model: model_id,
                                input: crate::input::Input::default(),
                                error: None,
                                show_plaintext: false,
                            });
                        } else {
                            selection_to_apply =
                                Some((provider_name, model_id, api_key_env, api_base));
                        }
                    }
                    close_dialog = true;
                }
                KeyCode::Up => {
                    dialog.move_up();
                    check_local_fetch = true;
                }
                KeyCode::Down => {
                    dialog.move_down();
                    check_local_fetch = true;
                }
                KeyCode::Left => dialog.switch_to_providers(),
                KeyCode::Right => dialog.switch_to_models(),
                KeyCode::Tab => {
                    if dialog.focus == DialogFocus::Providers {
                        dialog.switch_to_models();
                    } else {
                        dialog.switch_to_providers();
                    }
                }
                _ => {}
            }
        }

        if check_local_fetch {
            self.attempt_local_model_fetch();
        }

        if let Some(dialog) = need_api_key_dialog {
            self.state.api_key_dialog = Some(dialog);
        }
        if let Some((provider_name, model_id, api_key_env, api_base)) = selection_to_apply {
            persist_active_provider_selection(
                &mut self.state,
                provider_name,
                model_id,
                api_key_env,
                api_base,
            );
        }
        if close_dialog {
            self.state.input_mode = InputMode::Editing;
            self.state.provider_dialog = None;
        }
        Ok(())
    }

    fn open_load_snapshot_dialog(&mut self) {
        match list_session_snapshots() {
            Ok(catalog) if catalog.is_empty() => {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(
                        "No session snapshots found in ~/.xiaoo/session/.".to_string(),
                    ));
            }
            Ok(catalog) => {
                self.state.input_mode = InputMode::SessionSnapshotSelection;
                self.state.session_snapshot_dialog = Some(SessionSnapshotDialog::new(catalog));
            }
            Err(error) => self
                .state
                .chat_state
                .messages
                .push(crate::chat::Message::error(format!(
                    "Load snapshot failed: {error:#}"
                ))),
        }
        self.state.chat_state.stick_to_bottom = true;
    }

    fn open_sandbox_selection_dialog(&mut self) {
        if self.gateway.remote_base_url().is_some() {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(
                    "Remote backend is active. Use /remote off before switching local sandbox."
                        .to_string(),
                ));
            self.state.chat_state.stick_to_bottom = true;
            return;
        }
        let current = current_sandbox_id(&self.state.agent_config);
        self.state.input_mode = InputMode::SandboxSelection;
        self.state.sandbox_dialog = Some(SandboxDialog::new(current));
    }

    fn handle_sandbox_selection_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Editing;
                self.state.sandbox_dialog = None;
            }
            KeyCode::Up => {
                if let Some(dialog) = self.state.sandbox_dialog.as_mut() {
                    dialog.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(dialog) = self.state.sandbox_dialog.as_mut() {
                    dialog.move_down();
                }
            }
            KeyCode::Enter => {
                let selected = self
                    .state
                    .sandbox_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.selected())
                    .map(|option| (option.id, option.name));
                self.state.input_mode = InputMode::Editing;
                self.state.sandbox_dialog = None;
                if let Some((id, name)) = selected {
                    self.state.agent_config.operation_backend =
                        sandbox_backend_config(id, &self.state.agent_config.operation_backend);
                    self.state.status_panel.set_backend(sandbox_display_name(
                        &self.state.agent_config.operation_backend,
                    ));
                    self.state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::system(format!(
                            "Sandbox backend: {name}. Applies to new local sessions. Use /new to start one."
                        )));
                    self.state.chat_state.stick_to_bottom = true;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_session_snapshot_selection_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Editing;
                self.state.session_snapshot_dialog = None;
            }
            KeyCode::Up => {
                if let Some(dialog) = self.state.session_snapshot_dialog.as_mut() {
                    dialog.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(dialog) = self.state.session_snapshot_dialog.as_mut() {
                    dialog.move_down();
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(dialog) = self.state.session_snapshot_dialog.as_mut() {
                    dialog.toggle_pane();
                }
            }
            KeyCode::Enter => {
                let selected = self
                    .state
                    .session_snapshot_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.selected_entry())
                    .map(|entry| entry.snapshot_key.clone());
                self.state.input_mode = InputMode::Editing;
                self.state.session_snapshot_dialog = None;
                if let Some(snapshot_key) = selected {
                    match load_snapshot_by_key(&snapshot_key) {
                        Ok((snapshot, parent_chain)) => {
                            let name = snapshot.name.clone();
                            self.load_snapshot_into_state(
                                &snapshot_key,
                                &name,
                                snapshot,
                                parent_chain,
                            )
                            .await
                        }
                        Err(error) => {
                            self.state
                                .chat_state
                                .messages
                                .push(crate::chat::Message::error(format!(
                                    "Load snapshot failed: {error:#}"
                                )))
                        }
                    }
                    self.state.chat_state.stick_to_bottom = true;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn load_snapshot_into_state(
        &mut self,
        snapshot_key: &str,
        name: &str,
        snapshot: crate::session_snapshot_service::TuiSessionSnapshot,
        parent_chain: Vec<String>,
    ) {
        self.gateway.reset_for_new_session(&mut self.state);
        let snapshot_context = crate::session_snapshot_service::SnapshotContext::from_snapshot(
            snapshot_key.to_string(),
            &snapshot,
        );
        let record = apply_snapshot(&mut self.state, snapshot);
        let chain_display = if parent_chain.is_empty() {
            name.to_string()
        } else {
            format!("{} → {}", parent_chain.join(" → "), name)
        };
        self.state.current_snapshot_context = Some(snapshot_context);
        if let Some(record) = record {
            self.gateway.import_session_snapshot(record).await;
        }
        self.state
            .chat_state
            .messages
            .push(crate::chat::Message::system(format!(
                "Session snapshot loaded: {chain_display}"
            )));
    }

    fn open_provider_selection_dialog(&mut self) {
        self.state.input_mode = InputMode::ProviderSelection;
        self.state.provider_dialog = Some(ProviderDialog::new_with_selection(
            self.state.chat_state.available_providers.clone(),
            Some(&self.state.agent_config.llm.provider),
            Some(&self.state.agent_config.llm.model),
        ));
        self.attempt_local_model_fetch();
    }

    fn attempt_local_model_fetch(&mut self) {
        let should_fetch = self.state.provider_dialog.as_ref().map_or(false, |d| {
            !d.local_models_loading
                && d.providers.get(d.selected_provider).map_or(false, |p| {
                    p.name == "local" && p.models.len() == 1 && p.models[0].name.contains("(Local)")
                })
        });
        if !should_fetch {
            return;
        }
        if let Some(dialog) = self.state.provider_dialog.as_mut() {
            dialog.set_local_models_loading();
        }
        let api_base = crate::provider_service::default_api_base_for_provider("local");
        self.start_local_model_fetch(api_base);
    }

    async fn handle_turn_delete_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                if let Some(dialog) = self.state.delete_dialog.as_mut() {
                    if dialog.is_selecting() {
                        self.state.input_mode = InputMode::Editing;
                        self.state.delete_dialog = None;
                    } else {
                        self.state.delete_dialog =
                            DeleteDialog::new(&self.state.chat_state.messages);
                    }
                } else {
                    self.state.input_mode = InputMode::Editing;
                }
            }
            KeyCode::Up => {
                if let Some(dialog) = self.state.delete_dialog.as_mut() {
                    dialog.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(dialog) = self.state.delete_dialog.as_mut() {
                    dialog.move_down();
                }
            }
            KeyCode::Enter => {
                let action = match self.state.delete_dialog.as_mut() {
                    Some(dialog) => {
                        if dialog.is_selecting() {
                            dialog.advance_to_confirm();
                            None
                        } else {
                            dialog.selected_turn().map(|t| (t.msg_range, t.turn_index))
                        }
                    }
                    None => None,
                };

                if let Some(((start, end), turn_index)) = action {
                    self.state.input_mode = InputMode::Editing;
                    self.state.delete_dialog = None;

                    // 1. Remove from core's LoopState (true LLM context)
                    {
                        let session_id = self.state.session_id.clone();
                        let store = self.gateway.session_store_handle();
                        if let Some(mut record) = store.load(&session_id).await {
                            if let Some(ref mut snapshot) = record.loop_state {
                                crate::services::turn_delete::remove_turn_from_session_messages(
                                    &mut snapshot.messages,
                                    turn_index,
                                );
                            }
                            store.save(record).await;
                        }
                    }

                    // 2. Remove from TUI's local copy
                    crate::services::turn_delete::remove_turn_from_session_messages(
                        &mut self.state.session_messages,
                        turn_index,
                    );

                    // 3. Remove from TUI display
                    self.state.chat_state.messages.drain(start..end);
                    self.state.render_state = crate::app_state::RenderState::default();
                    self.state.chat_state.stick_to_bottom = true;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn open_cron_dialog(&mut self) {
        match crate::services::cron_service::load_cron_snapshot(&self.state.config_path) {
            Ok(snapshot) => {
                let dialog = crate::cron_dialog::CronDialog::new(
                    snapshot.jobs,
                    snapshot.jobs_file,
                    snapshot.default_timeout_secs,
                    snapshot.cron_section_present,
                );
                self.state.input_mode = InputMode::CronManagement;
                self.state.cron_dialog = Some(dialog);
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(
                        "Cron job management opened. Use arrow keys to navigate, 'a' to add, 'e' to edit, 'd' to delete, Space to toggle.".to_string(),
                    ));
                self.state.chat_state.stick_to_bottom = true;
            }
            Err(error) => {
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::error(format!(
                        "Failed to load cron config: {error:#}"
                    )));
                self.state.chat_state.stick_to_bottom = true;
            }
        }
    }

    async fn handle_cron_management_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(dialog) = self.state.cron_dialog.as_mut() else {
            self.state.input_mode = InputMode::Editing;
            return Ok(());
        };

        let mode = dialog.mode.clone();

        match &mode {
            CronDialogMode::List => match key.code {
                KeyCode::Esc => {
                    self.state.input_mode = InputMode::Editing;
                    self.state.cron_dialog = None;
                    self.state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::system(
                            "Cron job management closed. Restart daemon to apply changes."
                                .to_string(),
                        ));
                    self.state.chat_state.stick_to_bottom = true;
                }
                KeyCode::Up => {
                    dialog.move_up();
                }
                KeyCode::Down => {
                    dialog.move_down();
                }
                KeyCode::Char('a') => {
                    dialog.start_add();
                }
                KeyCode::Char('e') => {
                    dialog.start_edit();
                }
                KeyCode::Char('d') => {
                    dialog.start_delete_confirm();
                }
                KeyCode::Char(' ') => {
                    if let Err(e) = dialog.toggle_enabled() {
                        self.state
                            .chat_state
                            .messages
                            .push(crate::chat::Message::error(format!(
                                "Failed to toggle job: {e}"
                            )));
                        self.state.chat_state.stick_to_bottom = true;
                    }
                }
                _ => {}
            },
            CronDialogMode::ConfirmDelete { .. } => match key.code {
                KeyCode::Esc => {
                    dialog.back_to_list();
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Err(e) = dialog.confirm_delete() {
                        self.state
                            .chat_state
                            .messages
                            .push(crate::chat::Message::error(format!(
                                "Failed to delete job: {e}"
                            )));
                        self.state.chat_state.stick_to_bottom = true;
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    dialog.back_to_list();
                }
                _ => {}
            },
            CronDialogMode::EditForm { .. } => match key.code {
                KeyCode::Esc => {
                    dialog.back_to_list();
                }
                KeyCode::Tab if key.modifiers.contains(event::KeyModifiers::SHIFT) => {
                    dialog.edit_prev_field();
                }
                KeyCode::Tab => {
                    dialog.edit_next_field();
                }
                KeyCode::Enter => match dialog.save_edit_form() {
                    Ok(()) => {}
                    Err(error) => {
                        dialog.edit_set_error(error);
                    }
                },
                KeyCode::Backspace => {
                    dialog.edit_backspace();
                    dialog.edit_clear_error();
                }
                KeyCode::Char(c) => {
                    dialog.edit_push_char(c);
                    dialog.edit_clear_error();
                }
                _ => {}
            },
        }
        Ok(())
    }
}

/// Classifies whether a key event in editing mode inserts a newline (rather
/// than submitting or falling through to the editor's default text input).
///
/// Two key forms route to the same newline outcome:
/// - `Enter` with Alt or Control: most terminals send this for Alt+Enter /
///   Ctrl+Enter. Some emulators encode Ctrl+J as `Enter`+Control rather than
///   `Char('j')`+Control, so the Enter branch must accept Control too.
/// - `Char('j'|'J')` with Control (and without Shift): the traditional Unix
///   "LF" / ^J. Shift is excluded so Ctrl+Shift+J (and similar combos) is
///   not silently hijacked as a newline.
///
/// Not every terminal can deliver all of these; `Ctrl+J` is the documented
/// primary shortcut, the others are compatibility fallbacks.
fn editing_key_inserts_newline(code: KeyCode, modifiers: event::KeyModifiers) -> bool {
    match code {
        KeyCode::Enter => {
            modifiers.intersects(event::KeyModifiers::ALT | event::KeyModifiers::CONTROL)
        }
        KeyCode::Char('j') | KeyCode::Char('J') => {
            modifiers.contains(event::KeyModifiers::CONTROL)
                && !modifiers.contains(event::KeyModifiers::SHIFT)
        }
        _ => false,
    }
}

fn is_leave_subagent_view_key(key: &KeyEvent) -> bool {
    key.code == KeyCode::Left && key.modifiers.is_empty()
}

fn is_named_slash_command(trimmed: &str, command: &str) -> bool {
    let Some(first) = trimmed.split_whitespace().next() else {
        return false;
    };
    first.eq_ignore_ascii_case(command)
}

fn slash_command_argument<'a>(trimmed: &'a str, command: &str) -> Option<&'a str> {
    let first = trimmed.split_whitespace().next()?;
    if !first.eq_ignore_ascii_case(command) {
        return None;
    }
    let rest = trimmed[first.len()..].trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

fn normalize_remote_url_input(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.contains("://") || trimmed.is_empty() {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

/// Returns `(body, command_context)` when `trimmed` is a slash command
/// matching an external command from `~/.xiaoo/commands/`. The command
/// context bundles the resolved command name and raw arguments so callers
/// don't have to reassemble them and there is no positional-tuple footgun.
fn expand_external_command(
    trimmed: &str,
    external: &[crate::services::command_loader::ExternalCommand],
) -> Option<(String, agent_types::chat::CommandContext)> {
    let (cmd_name, user_args) = external_command_parts(trimmed)?;
    external
        .iter()
        .find(|cmd| cmd.name.eq_ignore_ascii_case(cmd_name))
        .map(|cmd| {
            let body = append_external_command_args(&cmd.body, user_args);
            (
                body,
                agent_types::chat::CommandContext {
                    command: cmd.name.clone(),
                    arguments: user_args.to_string(),
                },
            )
        })
}

fn external_command_parts(trimmed: &str) -> Option<(&str, &str)> {
    let first = trimmed.split_whitespace().next()?;
    let cmd_name = first.strip_prefix('/')?;
    if cmd_name.is_empty() {
        return None;
    }
    let user_args = trimmed[first.len()..].trim();
    Some((cmd_name, user_args))
}

fn append_external_command_args(body: &str, user_args: &str) -> String {
    if user_args.is_empty() {
        return body.to_string();
    }
    if body.is_empty() {
        return user_args.to_string();
    }
    format!("{body}\n\n{user_args}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::command_loader::ExternalCommand;

    fn external_commands() -> Vec<ExternalCommand> {
        vec![ExternalCommand {
            name: "review".to_string(),
            description: "Review code".to_string(),
            body: "Review this carefully.".to_string(),
        }]
    }

    #[test]
    fn editing_newline_key_plain_enter_is_not_newline() {
        assert!(!editing_key_inserts_newline(
            KeyCode::Enter,
            event::KeyModifiers::empty()
        ));
    }

    #[test]
    fn editing_newline_key_alt_enter_inserts_newline() {
        assert!(editing_key_inserts_newline(
            KeyCode::Enter,
            event::KeyModifiers::ALT
        ));
    }

    #[test]
    fn editing_newline_key_ctrl_enter_inserts_newline() {
        // Some terminal emulators encode Ctrl+J as Enter+Control rather
        // than Char('j')+Control, so the Enter branch must accept Control.
        assert!(editing_key_inserts_newline(
            KeyCode::Enter,
            event::KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn editing_newline_key_ctrl_j_inserts_newline() {
        assert!(editing_key_inserts_newline(
            KeyCode::Char('j'),
            event::KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn editing_newline_key_ctrl_uppercase_j_inserts_newline() {
        assert!(editing_key_inserts_newline(
            KeyCode::Char('J'),
            event::KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn editing_newline_key_ctrl_shift_j_does_not_insert_newline() {
        // Shift must opt out so Ctrl+Shift+J is not silently hijacked.
        assert!(!editing_key_inserts_newline(
            KeyCode::Char('j'),
            event::KeyModifiers::CONTROL | event::KeyModifiers::SHIFT
        ));
    }

    #[test]
    fn editing_newline_key_plain_j_is_not_newline() {
        assert!(!editing_key_inserts_newline(
            KeyCode::Char('j'),
            event::KeyModifiers::empty()
        ));
    }

    #[test]
    fn editing_newline_key_alt_j_is_not_newline() {
        // Only Alt+Enter (not Alt+J) is a newline shortcut.
        assert!(!editing_key_inserts_newline(
            KeyCode::Char('j'),
            event::KeyModifiers::ALT
        ));
    }

    #[test]
    fn plain_left_leaves_subagent_view() {
        assert!(is_leave_subagent_view_key(&KeyEvent::new(
            KeyCode::Left,
            event::KeyModifiers::empty()
        )));
    }

    #[test]
    fn modified_left_does_not_leave_subagent_view() {
        assert!(!is_leave_subagent_view_key(&KeyEvent::new(
            KeyCode::Left,
            event::KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn former_shift_up_shortcut_does_not_leave_subagent_view() {
        assert!(!is_leave_subagent_view_key(&KeyEvent::new(
            KeyCode::Up,
            event::KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn external_command_exact_match_expands_to_body() {
        assert_eq!(
            expand_external_command("/review", &external_commands()),
            Some((
                "Review this carefully.".to_string(),
                agent_types::chat::CommandContext {
                    command: "review".to_string(),
                    arguments: "".to_string()
                }
            ))
        );
    }

    #[test]
    fn external_command_appends_user_input_after_command_token() {
        assert_eq!(
            expand_external_command("/review src/main.rs 看一下边界条件", &external_commands()),
            Some((
                "Review this carefully.\n\nsrc/main.rs 看一下边界条件".to_string(),
                agent_types::chat::CommandContext {
                    command: "review".to_string(),
                    arguments: "src/main.rs 看一下边界条件".to_string()
                }
            ))
        );
    }

    #[test]
    fn external_command_match_only_uses_first_token() {
        assert_eq!(
            expand_external_command("/review-extra input", &external_commands()),
            None
        );
        assert_eq!(
            expand_external_command("/reviewer input", &external_commands()),
            None
        );
    }

    #[test]
    fn external_command_with_empty_body_uses_user_input() {
        let commands = vec![ExternalCommand {
            name: "ask".to_string(),
            description: String::new(),
            body: String::new(),
        }];
        assert_eq!(
            expand_external_command("/ask hello", &commands),
            Some((
                "hello".to_string(),
                agent_types::chat::CommandContext {
                    command: "ask".to_string(),
                    arguments: "hello".to_string()
                }
            ))
        );
    }
}
