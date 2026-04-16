use anyhow::Result;
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::chat::{default_provider_list, merge_config_provider, ChatState, MessageRole};
use crate::config::Config;
use crate::input::Input;
use crate::interaction_prompt::{InteractionPromptState, PromptRequest};
use crate::provider_dialog::ProviderDialog;
use crate::services::command_loader::{load_external_commands, ExternalCommand};
use crate::selection::TranscriptSelection;
use crate::slash_complete::{apply_slash_pick, candidates_for_prefix, slash_typed_prefix};
use crate::status_panel::StatusPanel;
use crate::theme::Theme;

#[derive(PartialEq)]
pub enum InputMode {
    Editing,
    ProviderSelection,
    InteractionPrompt,
}

pub struct ApiKeyDialogState {
    pub provider: String,
    pub model: String,
    pub input: Input,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolToggleRegion {
    pub message_index: usize,
    pub rect: Rect,
}

#[derive(Default)]
pub struct RenderState {
    pub messages_area: Option<Rect>,
    pub tool_toggle_regions: Vec<ToolToggleRegion>,
    pub slash_popup_inner: Option<Rect>,
    pub interaction_prompt_list_area: Option<Rect>,
    pub interaction_prompt_supplement_area: Option<Rect>,
    /// Cached plain-text content for each rendered line in the transcript.
    /// Rebuilt every frame by `render_chat`.
    pub line_texts: Vec<String>,
    /// Parallel to `line_texts`: `true` for the first line of every message
    /// entry (the "▎ Role  HH:MM:SS" header).  These lines are excluded from
    /// copied text even when visually highlighted.
    pub line_is_header: Vec<bool>,
}

#[derive(Default)]
pub struct SlashState {
    pub selected: usize,
    pub dismissed: bool,
}

pub struct AppState {
    pub theme: Theme,
    pub chat_state: ChatState,
    pub status_panel: StatusPanel,
    pub input_mode: InputMode,
    pub should_quit: bool,
    pub provider_dialog: Option<ProviderDialog>,
    pub api_key_dialog: Option<ApiKeyDialogState>,
    pub loading_tick: usize,
    pub agent_config: Config,
    pub config_path: PathBuf,
    pub workspace: PathBuf,
    pub session_messages: Vec<llm_client::ChatMessage>,
    pub session_id: String,
    pub slash: SlashState,
    pub interaction_prompt: Option<InteractionPromptState>,
    pub render_state: RenderState,
    /// Active text selection in the transcript area, if any.
    pub transcript_selection: Option<TranscriptSelection>,
    /// Set when text is copied to clipboard; drives the toast notification.
    pub copy_notice: Option<Instant>,
    pub external_commands: Vec<ExternalCommand>,
}

impl AppState {
    pub fn new(config_path: PathBuf, workspace: PathBuf) -> Result<Self, anyhow::Error> {
        Ok(Self {
            theme: Theme::default(),
            chat_state: ChatState::new(),
            status_panel: StatusPanel::new(),
            input_mode: InputMode::Editing,
            should_quit: false,
            provider_dialog: None,
            api_key_dialog: None,
            loading_tick: 0,
            agent_config: Config::default(),
            config_path,
            workspace,
            session_messages: Vec::new(),
            session_id: uuid::Uuid::new_v4().to_string(),
            slash: SlashState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            transcript_selection: None,
            copy_notice: None,
            external_commands: load_external_commands(),
        })
    }

    pub fn new_with_config(
        config: &Config,
        config_path: PathBuf,
        workspace: PathBuf,
    ) -> Result<Self, anyhow::Error> {
        let provider_name = config.llm.provider.clone();
        let model = config.llm.model.clone();
        let list = merge_config_provider(default_provider_list(), &provider_name, &model);
        let mut chat_state = ChatState::new();
        chat_state.available_providers = list;

        let mut status_panel = StatusPanel::new();
        let has_runtime_selection = !provider_name.trim().is_empty() && !model.trim().is_empty();
        if has_runtime_selection {
            status_panel.set_provider(&provider_name, &model);
            chat_state.messages.push(crate::chat::Message::system(format!(
                "Configured backend {} / {} from config. Messages now go through gateway/session interfaces.",
                provider_name, model
            )));
        }

        Ok(Self {
            theme: Theme::default(),
            chat_state,
            status_panel,
            input_mode: InputMode::Editing,
            should_quit: false,
            provider_dialog: None,
            api_key_dialog: None,
            loading_tick: 0,
            agent_config: config.clone(),
            config_path,
            workspace,
            session_messages: Vec::new(),
            session_id: uuid::Uuid::new_v4().to_string(),
            slash: SlashState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            external_commands: load_external_commands(),
            transcript_selection: None,
            copy_notice: None,
        })
    }

    /// Mark that text was just copied; shows the toast for 1.5 s.
    pub fn set_copy_notice(&mut self) {
        self.copy_notice = Some(Instant::now());
    }

    /// Returns `true` while the copy toast should still be visible.
    pub fn copy_notice_active(&self) -> bool {
        self.copy_notice
            .map(|t| t.elapsed() < Duration::from_millis(1500))
            .unwrap_or(false)
    }

    /// Extract the plain text covered by the current transcript selection.
    /// Returns `None` if there is no active selection or the selection is empty.
    ///
    /// Role-header lines ("▎ You  HH:MM:SS" etc.) are excluded from the result
    /// even when they fall inside the highlighted range.
    pub fn transcript_selected_text(&self) -> Option<String> {
        let sel = self.transcript_selection.as_ref()?;
        if sel.is_empty() {
            return None;
        }
        let (start_line, start_col, end_line, end_col) = sel.normalised();
        let lines = &self.render_state.line_texts;

        if start_line >= lines.len() {
            return None;
        }

        let mut segments: Vec<String> = Vec::new();
        for line_idx in start_line..=end_line.min(lines.len().saturating_sub(1)) {
            // Skip role/tool/planner header lines (▎ Role  HH:MM:SS).
            if self.render_state.line_is_header.get(line_idx).copied().unwrap_or(false) {
                continue;
            }
            let line = &lines[line_idx];
            let col_start = if line_idx == start_line { start_col } else { 0 };
            let col_end = if line_idx == end_line {
                end_col.min(line.chars().count())
            } else {
                line.chars().count()
            };
            let segment: String = line
                .chars()
                .skip(col_start)
                .take(col_end.saturating_sub(col_start))
                .collect();
            segments.push(segment);
        }

        let result = segments.join("\n");
        let result = result.trim_matches('\n');
        if result.is_empty() { None } else { Some(result.to_owned()) }
    }

    pub fn open_interaction_prompt(
        &mut self,
        req: PromptRequest,
        allow_while_loading: bool,
    ) -> Result<(), String> {
        if self.chat_state.is_loading && !allow_while_loading {
            return Err("交互不可用：正在流式输出".to_string());
        }
        if req.choices.is_empty() {
            return Err("choices 不能为空".to_string());
        }
        let state = InteractionPromptState::new(req).ok_or_else(|| "invalid prompt".to_string())?;
        self.interaction_prompt = Some(state);
        self.input_mode = InputMode::InteractionPrompt;
        Ok(())
    }

    pub fn slash_menu_visible(&self) -> bool {
        if self.interaction_prompt.is_some() {
            return false;
        }
        if self.input_mode != InputMode::Editing || self.chat_state.is_loading {
            return false;
        }
        if self.slash.dismissed {
            return false;
        }
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        let Some(prefix) = slash_typed_prefix(value, cursor) else {
            return false;
        };
        !candidates_for_prefix(&prefix, &self.external_commands).is_empty()
    }

    pub fn slash_popup_height(&self) -> u16 {
        if !self.slash_menu_visible() {
            return 0;
        }
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        let Some(prefix) = slash_typed_prefix(value, cursor) else {
            return 0;
        };
        let candidate_count = candidates_for_prefix(&prefix, &self.external_commands)
            .len()
            .min(6);
        if candidate_count == 0 {
            return 0;
        }
        candidate_count as u16 + 2
    }

    pub fn slash_candidate_count(&self) -> usize {
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        slash_typed_prefix(value, cursor)
            .map(|prefix| candidates_for_prefix(&prefix, &self.external_commands).len())
            .unwrap_or(0)
    }

    pub fn note_input_changed(&mut self) {
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        if slash_typed_prefix(value, cursor).is_none() {
            self.slash.dismissed = false;
        }
        let candidate_count = self.slash_candidate_count();
        if candidate_count == 0 {
            return;
        }
        self.slash.selected = self.slash.selected.min(candidate_count - 1);
    }

    pub fn apply_slash_selection(&mut self) {
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        if let Some(prefix) = slash_typed_prefix(value, cursor) {
            let candidates = candidates_for_prefix(&prefix, &self.external_commands);
            if let Some(chosen) = candidates.get(self.slash.selected) {
                apply_slash_pick(&mut self.chat_state.input, chosen);
                self.note_input_changed();
            }
        }
    }

    pub fn get_last_assistant_content(&self) -> Option<String> {
        self.chat_state
            .messages
            .iter()
            .rev()
            .find(|message| {
                message.role == MessageRole::Assistant
                    && !message.is_streaming
                    && !message.content.is_empty()
            })
            .map(|message| message.content.clone())
    }
}
