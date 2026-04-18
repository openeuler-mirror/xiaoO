use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};

use crate::app::App;
use crate::app_state::{ApiKeyDialogState, InputMode};
use crate::input::EventHandler;
use crate::interaction_prompt::{PromptFocus, PromptResolution};
use crate::provider_dialog::{DialogFocus, ProviderDialog};
use crate::provider_service::{
    copy_to_clipboard, persist_active_provider_selection, persisted_selection_settings,
    validate_and_connect_api_key,
};
use crate::skills_service::render_skills_overview;
use crate::workspace_service::{first_token_is_dir_command, resolve_dir_command};

impl App {
    pub(crate) async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return Ok(());
        }

        // Ctrl+C: copy selected input text when selection exists, otherwise quit.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL)
            || key.code == KeyCode::Char('\x03')
        {
            // Check input selection first.
            if let Some(text) = self
                .state
                .chat_state
                .input
                .selected_text()
                .map(str::to_owned)
            {
                self.state.chat_state.input.clear_selection();
                if let Err(e) = copy_to_clipboard(&text) {
                    tracing::warn!("copy_to_clipboard failed: {}", e);
                }
                return Ok(());
            }
            // Check transcript selection.
            if let Some(text) = self.state.transcript_selected_text() {
                self.state.transcript_selection = None;
                if let Err(e) = copy_to_clipboard(&text) {
                    tracing::warn!("copy_to_clipboard failed: {}", e);
                }
                return Ok(());
            }
            // No selection → quit.
            self.state.should_quit = true;
            return Ok(());
        }

        // Ctrl+X: cut selected input text.
        if key.code == KeyCode::Char('x') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
            if let Some(text) = self.state.chat_state.input.delete_selected() {
                if let Err(e) = copy_to_clipboard(&text) {
                    tracing::warn!("copy_to_clipboard failed: {}", e);
                }
                self.state.note_input_changed();
            }
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

        match self.state.input_mode {
            InputMode::Editing => self.handle_editing_mode_key(key).await,
            InputMode::ProviderSelection => self.handle_provider_selection_key(key),
            InputMode::InteractionPrompt => Ok(()),
        }
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
                self.state.note_input_changed();
            } else {
                self.state.cycle_agent_role(false);
            }
            return Ok(());
        }

        if key.code == KeyCode::BackTab {
            if crate::slash_complete::slash_typed_prefix(
                self.state.chat_state.input.value(),
                self.state.chat_state.input.cursor(),
            )
            .is_none()
            {
                self.state.cycle_agent_role(true);
                return Ok(());
            }
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
                    self.state.slash.dismissed = true;
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.state.slash.dismissed = true;
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
            KeyCode::Enter => self.submit_editing_input().await?,
            _ => {
                self.state.chat_state.input.handle_event(&Event::Key(key));
                self.state.note_input_changed();
            }
        }
        Ok(())
    }

    async fn submit_editing_input(&mut self) -> Result<()> {
        let user_input = self.state.chat_state.input.value().to_string();
        if user_input.trim().is_empty() {
            return Ok(());
        }

        let trimmed = user_input.trim();

        if trimmed.eq_ignore_ascii_case("/new") {
            self.gateway.reset_for_new_session(&mut self.state);
            self.state.reset_for_new_session();
            return Ok(());
        }

        if self.state.chat_state.is_loading {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(
                    "当前任务仍在运行。请等待它结束，或先按 Esc 取消，再发送新消息。".to_string(),
                ));
            self.state.chat_state.stick_to_bottom = true;
            return Ok(());
        }

        if trimmed.eq_ignore_ascii_case("/connect") {
            self.state.chat_state.input.reset();
            self.open_provider_selection_dialog();
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

        if first_token_is_dir_command(trimmed) {
            match resolve_dir_command(trimmed, &self.state.workspace) {
                Ok(path) => {
                    self.state.workspace = path;
                    self.state.status_panel.set_workspace(&self.state.workspace);
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

        // NOTE: /create-skill is not yet implemented; disabled until ready.
        // if user_input.trim().starts_with("/create-skill") { ... }

        // External commands from ~/.xiaoo/command/
        {
            let cmd_name = trimmed.strip_prefix('/').unwrap_or("");
            if let Some(cmd) = self
                .state
                .external_commands
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(cmd_name))
            {
                let body = cmd.body.clone();
                self.state.chat_state.input.reset();
                if let Err(error) = self.gateway.start_turn(&mut self.state, body).await {
                    self.state
                        .chat_state
                        .messages
                        .push(crate::chat::Message::system(format!(
                            "无法启动当前请求: {error}"
                        )));
                    self.state.chat_state.stick_to_bottom = true;
                }
                return Ok(());
            }
        }

        if let Err(error) = self.gateway.start_turn(&mut self.state, user_input).await {
            self.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(format!(
                    "无法启动当前请求: {error}"
                )));
            self.state.chat_state.stick_to_bottom = true;
        }
        Ok(())
    }

    fn handle_provider_selection_key(&mut self, key: KeyEvent) -> Result<()> {
        let mut selection_to_apply = None;
        let mut need_api_key_dialog = None;
        let mut close_dialog = false;

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
                            });
                        } else {
                            selection_to_apply =
                                Some((provider_name, model_id, api_key_env, api_base));
                        }
                    }
                    close_dialog = true;
                }
                KeyCode::Up => dialog.move_up(),
                KeyCode::Down => dialog.move_down(),
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

    fn open_provider_selection_dialog(&mut self) {
        self.state.input_mode = InputMode::ProviderSelection;
        self.state.provider_dialog = Some(ProviderDialog::new_with_selection(
            self.state.chat_state.available_providers.clone(),
            Some(&self.state.agent_config.llm.provider),
            Some(&self.state.agent_config.llm.model),
        ));
    }
}
