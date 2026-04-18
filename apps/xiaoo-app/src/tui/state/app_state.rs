use anyhow::Result;
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::chat::{default_provider_list, merge_config_provider, ChatState, MessageRole};
use crate::config::{AgentRoleConfig, Config};
use crate::input::Input;
use crate::interaction_prompt::{InteractionPromptState, PromptRequest};
use crate::provider_dialog::ProviderDialog;
use crate::selection::TranscriptSelection;
use crate::services::command_loader::{load_external_commands, ExternalCommand};
use crate::slash_complete::{apply_slash_pick, candidates_for_prefix, slash_typed_prefix};
use crate::status_panel::StatusPanel;
use crate::theme::Theme;

#[derive(PartialEq)]
pub enum InputMode {
    Editing,
    ProviderSelection,
    InteractionPrompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatusLight {
    Idle,
    Running,
    AwaitingInteraction,
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
    pub active_agent_role: Option<String>,
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
            chat_state: build_chat_state(&Config::default()),
            status_panel: build_status_panel(&Config::default()),
            input_mode: InputMode::Editing,
            should_quit: false,
            provider_dialog: None,
            api_key_dialog: None,
            loading_tick: 0,
            agent_config: Config::default(),
            active_agent_role: None,
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
        Ok(Self {
            theme: Theme::default(),
            chat_state: build_chat_state(config),
            status_panel: build_status_panel(config),
            input_mode: InputMode::Editing,
            should_quit: false,
            provider_dialog: None,
            api_key_dialog: None,
            loading_tick: 0,
            agent_config: config.clone(),
            active_agent_role: None,
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

    pub fn reset_for_new_session(&mut self) {
        self.chat_state = build_chat_state(&self.agent_config);
        self.status_panel = build_status_panel(&self.agent_config);
        self.status_panel.set_workspace(&self.workspace);
        self.input_mode = InputMode::Editing;
        self.provider_dialog = None;
        self.api_key_dialog = None;
        self.loading_tick = 0;
        self.session_messages.clear();
        self.session_id = uuid::Uuid::new_v4().to_string();
        self.slash = SlashState::default();
        self.interaction_prompt = None;
        self.render_state = RenderState::default();
        self.transcript_selection = None;
        self.copy_notice = None;
        self.external_commands = load_external_commands();
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
            if self
                .render_state
                .line_is_header
                .get(line_idx)
                .copied()
                .unwrap_or(false)
            {
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
        if result.is_empty() {
            None
        } else {
            Some(result.to_owned())
        }
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

    pub fn agent_tab_labels(&self) -> Vec<String> {
        let mut tabs = vec!["Chat".to_string()];
        tabs.extend(self.agent_config.agent_role_ids());
        tabs
    }

    pub fn active_agent_tab_label(&self) -> &str {
        self.active_agent_role.as_deref().unwrap_or("Chat")
    }

    pub fn active_agent_role_config(&self) -> Option<&AgentRoleConfig> {
        self.active_agent_role
            .as_deref()
            .and_then(|role_id| self.agent_config.agent_role(role_id))
    }

    pub fn cycle_agent_role(&mut self, reverse: bool) -> bool {
        let role_ids = self.agent_config.agent_role_ids();
        if role_ids.is_empty() {
            return false;
        }

        let total_tabs = role_ids.len() + 1;
        let current_index = self
            .active_agent_role
            .as_ref()
            .and_then(|current| role_ids.iter().position(|role_id| role_id == current))
            .map(|index| index + 1)
            .unwrap_or(0);
        let next_index = if reverse {
            (current_index + total_tabs - 1) % total_tabs
        } else {
            (current_index + 1) % total_tabs
        };

        self.active_agent_role = if next_index == 0 {
            None
        } else {
            role_ids.get(next_index - 1).cloned()
        };
        true
    }

    pub fn runtime_status_light(&self) -> RuntimeStatusLight {
        if self.interaction_prompt.is_some() {
            RuntimeStatusLight::AwaitingInteraction
        } else if self.chat_state.is_loading {
            RuntimeStatusLight::Running
        } else {
            RuntimeStatusLight::Idle
        }
    }
}

fn build_chat_state(config: &Config) -> ChatState {
    let provider_name = config.llm.provider.clone();
    let model = config.llm.model.clone();
    let mut chat_state = ChatState::new();
    chat_state.available_providers =
        merge_config_provider(default_provider_list(), &provider_name, &model);

    if !provider_name.trim().is_empty() && !model.trim().is_empty() {
        chat_state.messages.push(crate::chat::Message::system(format!(
            "Configured backend {} / {} from config. Messages now go through gateway/session interfaces.",
            provider_name, model
        )));
    }

    chat_state
}

fn build_status_panel(config: &Config) -> StatusPanel {
    let mut status_panel = StatusPanel::new();
    if !config.llm.provider.trim().is_empty() && !config.llm.model.trim().is_empty() {
        status_panel.set_provider(&config.llm.provider, &config.llm.model);
    }
    status_panel
}

#[cfg(test)]
mod tests {
    use super::{AppState, RuntimeStatusLight};
    use crate::interaction_prompt::{PromptChoice, PromptRequest};
    use crate::selection::TranscriptSelection;
    use agent_types::{ChatMessage, ContentBlock, MessageRole};
    use std::path::PathBuf;

    #[test]
    fn runtime_status_light_is_idle_by_default() {
        let state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        assert_eq!(state.runtime_status_light(), RuntimeStatusLight::Idle);
    }

    #[test]
    fn runtime_status_light_is_running_while_loading() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        state.chat_state.is_loading = true;
        assert_eq!(state.runtime_status_light(), RuntimeStatusLight::Running);
    }

    #[test]
    fn runtime_status_light_prefers_interaction_when_prompt_is_open() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        state.chat_state.is_loading = true;
        state
            .open_interaction_prompt(sample_prompt_request(), true)
            .expect("interaction prompt should open");
        assert_eq!(
            state.runtime_status_light(),
            RuntimeStatusLight::AwaitingInteraction
        );
    }

    fn sample_prompt_request() -> PromptRequest {
        PromptRequest {
            request_id: "demo-1".to_string(),
            title: "示例交互".to_string(),
            body: Some("请选择一个选项（可填写补充说明）。".to_string()),
            choices: vec![PromptChoice {
                id: "a".to_string(),
                label: "选项 A".to_string(),
                description: Some("快速路径".to_string()),
            }],
            allow_custom_input: true,
            multi_select: false,
            default_index: Some(0),
        }
    }
}
