use anyhow::Result;
use ratatui::{layout::Rect, text::Line};
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

#[derive(Clone)]
pub struct ApiKeyDialogState {
    pub provider: String,
    pub model: String,
    pub input: Input,
    pub error: Option<String>,
    pub show_plaintext: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolToggleRegion {
    pub message_index: usize,
    pub rect: Rect,
}

#[derive(Clone)]
pub struct CachedMessageRender {
    pub revision: u64,
    pub width: u16,
    pub theme: Theme,
    pub lines: Vec<Line<'static>>,
    pub visual_line_count: usize,
    pub tool_toggle_row_offset: Option<usize>,
}

#[derive(Clone)]
pub struct CachedMessageLayout {
    pub message_index: usize,
    pub start_visual_row: usize,
    pub tool_toggle_row_offset: Option<usize>,
}

#[derive(Clone)]
pub struct TranscriptRenderCache {
    pub all_lines: Vec<Line<'static>>,
    pub visual_lines: Vec<Line<'static>>,
    pub line_texts: Vec<String>,
    pub line_is_header: Vec<bool>,
    pub logical_line_visual_starts: Vec<usize>,
    pub message_layouts: Vec<CachedMessageLayout>,
    pub total_lines: usize,
}

#[derive(Default)]
pub struct RenderState {
    pub messages_area: Option<Rect>,
    pub theme_toggle_area: Option<Rect>,
    pub api_key_toggle_area: Option<Rect>,
    pub message_renders: Vec<Option<CachedMessageRender>>,
    pub transcript_cache: Option<TranscriptRenderCache>,
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
    pub dismissed_prefix: Option<String>,
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

    pub fn toggle_theme(&mut self) {
        self.theme = self.theme.toggled();
    }

    pub fn toggle_api_key_visibility(&mut self) {
        if let Some(dialog) = self.api_key_dialog.as_mut() {
            dialog.show_plaintext = !dialog.show_plaintext;
        }
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
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        let Some(prefix) = slash_typed_prefix(value, cursor) else {
            return false;
        };
        if self
            .slash
            .dismissed_prefix
            .as_deref()
            .is_some_and(|dismissed| dismissed == prefix)
        {
            return false;
        }
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
        let prefix = slash_typed_prefix(value, cursor);
        if self
            .slash
            .dismissed_prefix
            .as_deref()
            .is_some_and(|dismissed| prefix.as_deref() != Some(dismissed))
        {
            self.slash.dismissed_prefix = None;
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

    pub fn dismiss_current_slash_menu(&mut self) {
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        self.slash.dismissed_prefix = slash_typed_prefix(value, cursor);
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
    use super::{ApiKeyDialogState, AppState, RuntimeStatusLight};
    use crate::input::Input;
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

    #[test]
    fn toggle_theme_switches_between_dark_and_light() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        let initial_is_light = state.theme.is_light();

        state.toggle_theme();
        assert_ne!(state.theme.is_light(), initial_is_light);

        state.toggle_theme();
        assert_eq!(state.theme.is_light(), initial_is_light);
    }

    #[test]
    fn toggle_api_key_visibility_switches_between_hidden_and_plaintext() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        state.api_key_dialog = Some(ApiKeyDialogState {
            provider: "demo".to_string(),
            model: "model".to_string(),
            input: Input::default(),
            error: None,
            show_plaintext: false,
        });

        state.toggle_api_key_visibility();
        assert!(state
            .api_key_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.show_plaintext));

        state.toggle_api_key_visibility();
        assert!(state
            .api_key_dialog
            .as_ref()
            .is_some_and(|dialog| !dialog.show_plaintext));
    }

    #[test]
    fn slash_menu_reopens_for_new_prefix_after_dismiss() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        state.chat_state.input = "/skills".into();

        assert!(state.slash_menu_visible());

        state.dismiss_current_slash_menu();
        assert!(!state.slash_menu_visible());

        state.chat_state.input = "/".into();
        state.note_input_changed();
        assert!(state.slash_menu_visible());
    }

    #[test]
    fn slash_menu_reopens_when_prefix_changes_after_escape() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        state.chat_state.input = "/c".into();

        assert!(state.slash_menu_visible());

        state.dismiss_current_slash_menu();
        assert!(!state.slash_menu_visible());

        state.chat_state.input = "/co".into();
        state.note_input_changed();
        assert!(state.slash_menu_visible());
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
