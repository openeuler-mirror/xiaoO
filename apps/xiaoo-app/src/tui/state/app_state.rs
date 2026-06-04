use agent_types::ReasoningEffort;
use anyhow::Result;
use ratatui::{layout::Rect, text::Line};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::chat::{default_provider_list, merge_config_provider, ChatState, TodoMessageState};
use crate::config::{AgentRoleConfig, Config};
use crate::gateway::backend::GatewayBackendConfig;
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
    SandboxSelection,
    SessionSnapshotSelection,
    InteractionPrompt,
    TurnDelete,
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

#[derive(Debug, Clone)]
pub struct SandboxOption {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone)]
pub struct SandboxDialog {
    pub options: Vec<SandboxOption>,
    pub selected: usize,
}

impl SandboxDialog {
    pub fn new(current_id: &str) -> Self {
        let mut options = vec![SandboxOption {
            id: "local",
            name: "Local",
            description: "本地执行，不启用 Seatbelt policy。",
        }];
        if cfg!(target_os = "macos") {
            options.push(SandboxOption {
                id: "seatbelt",
                name: "Seatbelt",
                description: "macOS sandbox-exec + local file policy。",
            });
        }
        if cfg!(target_os = "linux") {
            options.push(SandboxOption {
                id: "bubblewrap",
                name: "Bubblewrap",
                description: "Linux bubblewrap + local file policy。",
            });
        }
        let selected = options
            .iter()
            .position(|option| option.id == current_id)
            .unwrap_or(0);
        Self { options, selected }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        self.selected = (self.selected + 1).min(self.options.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&SandboxOption> {
        self.options.get(self.selected)
    }
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionFileChangeStats {
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFileChangeEntry {
    pub file_path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolFileBaseline {
    file_path: String,
    absolute_path: PathBuf,
}

pub struct AppState {
    pub theme: Theme,
    pub chat_state: ChatState,
    pub status_panel: StatusPanel,
    pub input_mode: InputMode,
    pub should_quit: bool,
    pub provider_dialog: Option<ProviderDialog>,
    pub sandbox_dialog: Option<SandboxDialog>,
    pub session_snapshot_dialog: Option<crate::session_snapshot_service::SessionSnapshotDialog>,
    pub delete_dialog: Option<crate::services::turn_delete::DeleteDialog>,
    pub api_key_dialog: Option<ApiKeyDialogState>,
    pub loading_tick: usize,
    pub agent_config: Config,
    pub active_agent_role: Option<String>,
    pub reasoning_effort: ReasoningEffort,
    pub config_path: PathBuf,
    pub workspace: PathBuf,
    pub session_messages: Vec<llm_client::ChatMessage>,
    pub plan_state: Option<TodoMessageState>,
    pub session_id: String,
    pub current_snapshot_context: Option<crate::session_snapshot_service::SnapshotContext>,
    pub slash: SlashState,
    pub interaction_prompt: Option<InteractionPromptState>,
    pub render_state: RenderState,
    /// Active text selection in the transcript area, if any.
    pub transcript_selection: Option<TranscriptSelection>,
    /// Set when text is copied to clipboard; drives the toast notification.
    pub copy_notice: Option<Instant>,
    pub external_commands: Vec<ExternalCommand>,
    pub session_file_changes: BTreeMap<String, SessionFileChangeStats>,
    pub tool_file_changes: HashMap<String, crate::chat::FileChangeDelta>,
    tool_file_baselines: HashMap<String, ToolFileBaseline>,
    session_file_content_baselines: HashMap<String, Option<String>>,
}

impl AppState {
    #[cfg(test)]
    pub fn new(config_path: PathBuf, workspace: PathBuf) -> Result<Self, anyhow::Error> {
        Ok(Self {
            theme: Theme::default(),
            chat_state: build_chat_state(&Config::default()),
            status_panel: build_status_panel(&Config::default()),
            input_mode: InputMode::Editing,
            should_quit: false,
            provider_dialog: None,
            sandbox_dialog: None,
            session_snapshot_dialog: None,
            delete_dialog: None,
            api_key_dialog: None,
            loading_tick: 0,
            agent_config: Config::default(),
            active_agent_role: None,
            reasoning_effort: Config::default().llm.reasoning_effort,
            config_path,
            workspace,
            session_messages: Vec::new(),
            plan_state: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            current_snapshot_context: None,
            slash: SlashState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            transcript_selection: None,
            copy_notice: None,
            external_commands: load_external_commands(),
            session_file_changes: BTreeMap::new(),
            tool_file_changes: HashMap::new(),
            tool_file_baselines: HashMap::new(),
            session_file_content_baselines: HashMap::new(),
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
            sandbox_dialog: None,
            session_snapshot_dialog: None,
            delete_dialog: None,
            api_key_dialog: None,
            loading_tick: 0,
            agent_config: config.clone(),
            active_agent_role: None,
            reasoning_effort: config.llm.reasoning_effort,
            config_path,
            workspace,
            session_messages: Vec::new(),
            plan_state: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            current_snapshot_context: None,
            slash: SlashState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            external_commands: load_external_commands(),
            transcript_selection: None,
            copy_notice: None,
            session_file_changes: BTreeMap::new(),
            tool_file_changes: HashMap::new(),
            tool_file_baselines: HashMap::new(),
            session_file_content_baselines: HashMap::new(),
        })
    }

    pub fn reset_for_new_session(&mut self) {
        self.chat_state = build_chat_state(&self.agent_config);
        self.status_panel = build_status_panel(&self.agent_config);
        self.status_panel.set_workspace(&self.workspace);
        self.input_mode = InputMode::Editing;
        self.provider_dialog = None;
        self.sandbox_dialog = None;
        self.session_snapshot_dialog = None;
        self.delete_dialog = None;
        self.api_key_dialog = None;
        self.loading_tick = 0;
        self.session_messages.clear();
        self.plan_state = None;
        self.session_id = uuid::Uuid::new_v4().to_string();
        self.current_snapshot_context = None;
        self.slash = SlashState::default();
        self.reasoning_effort = ReasoningEffort::default();
        self.interaction_prompt = None;
        self.render_state = RenderState::default();
        self.transcript_selection = None;
        self.copy_notice = None;
        self.external_commands = load_external_commands();
        self.session_file_changes.clear();
        self.tool_file_changes.clear();
        self.tool_file_baselines.clear();
        self.session_file_content_baselines.clear();
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

    pub fn reconcile_tool_file_change(
        &mut self,
        call_id: &str,
        next: Option<crate::chat::FileChangeDelta>,
    ) {
        if let Some(previous) = self.tool_file_changes.remove(call_id) {
            self.adjust_session_file_change(
                &previous.file_path,
                previous.additions,
                previous.deletions,
                false,
            );
        }

        let Some(next) = next.filter(|change| change.additions > 0 || change.deletions > 0) else {
            return;
        };

        self.adjust_session_file_change(&next.file_path, next.additions, next.deletions, true);
        self.tool_file_changes.insert(call_id.to_string(), next);
    }

    pub fn capture_tool_file_baseline(&mut self, call_id: &str, tool: &str, args_preview: &str) {
        if self.tool_file_baselines.contains_key(call_id) {
            return;
        }
        let Some(file_path) = parse_tool_target_file_path(tool, args_preview) else {
            return;
        };
        let absolute_path = resolve_workspace_file_path(&self.workspace, &file_path);
        let current_content = read_file_if_exists(&absolute_path);
        self.session_file_content_baselines
            .entry(file_path.clone())
            .or_insert_with(|| current_content.clone());
        self.tool_file_baselines.insert(
            call_id.to_string(),
            ToolFileBaseline {
                file_path,
                absolute_path,
            },
        );
    }

    pub fn reconcile_tool_file_change_from_baseline(
        &mut self,
        call_id: &str,
        fallback: Option<crate::chat::FileChangeDelta>,
    ) {
        let Some(baseline) = self.tool_file_baselines.remove(call_id) else {
            if !self.tool_file_changes.contains_key(call_id) {
                self.reconcile_tool_file_change(call_id, fallback);
            }
            return;
        };

        let current_content = read_file_if_exists(&baseline.absolute_path);
        let computed = self
            .session_file_content_baselines
            .get(&baseline.file_path)
            .and_then(|initial_content| {
                if initial_content.is_none() && current_content.is_none() {
                    None
                } else {
                    Some(file_content_delta(
                        &baseline.file_path,
                        initial_content.as_deref(),
                        current_content.as_deref(),
                    ))
                }
            });
        if let Some(delta) = computed.or(fallback) {
            self.session_file_changes.insert(
                delta.file_path.clone(),
                SessionFileChangeStats {
                    additions: delta.additions,
                    deletions: delta.deletions,
                },
            );
            if delta.additions == 0 && delta.deletions == 0 {
                self.session_file_changes.remove(&delta.file_path);
            }
        } else {
            self.reconcile_tool_file_change(call_id, None);
        }
    }

    pub fn discard_tool_file_baseline(&mut self, call_id: &str) {
        self.tool_file_baselines.remove(call_id);
    }

    pub fn clear_tool_file_baselines(&mut self) {
        self.tool_file_baselines.clear();
    }

    pub fn sorted_session_file_changes(&self) -> Vec<SessionFileChangeEntry> {
        let mut entries = self
            .session_file_changes
            .iter()
            .map(|(file_path, stats)| SessionFileChangeEntry {
                file_path: file_path.clone(),
                additions: stats.additions,
                deletions: stats.deletions,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            let left_total = left.additions + left.deletions;
            let right_total = right.additions + right.deletions;
            right_total
                .cmp(&left_total)
                .then(right.additions.cmp(&left.additions))
                .then(left.file_path.cmp(&right.file_path))
        });
        entries
    }

    pub fn display_file_path(&self, file_path: &str) -> String {
        let path = Path::new(file_path);
        if let Ok(relative) = path.strip_prefix(&self.workspace) {
            let display = relative.display().to_string();
            if !display.is_empty() {
                return display;
            }
        }
        file_path.to_string()
    }

    fn adjust_session_file_change(
        &mut self,
        file_path: &str,
        additions: u32,
        deletions: u32,
        add: bool,
    ) {
        let entry = self
            .session_file_changes
            .entry(file_path.to_string())
            .or_default();
        if add {
            entry.additions = entry.additions.saturating_add(additions);
            entry.deletions = entry.deletions.saturating_add(deletions);
        } else {
            entry.additions = entry.additions.saturating_sub(additions);
            entry.deletions = entry.deletions.saturating_sub(deletions);
        }

        if entry.additions == 0 && entry.deletions == 0 {
            self.session_file_changes.remove(file_path);
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
                self.chat_state.reset_input_history_navigation();
                self.note_input_changed();
            }
        }
    }

    pub fn dismiss_current_slash_menu(&mut self) {
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();
        self.slash.dismissed_prefix = slash_typed_prefix(value, cursor);
    }

    pub fn agent_tab_labels(&self) -> Vec<String> {
        self.agent_tabs()
            .into_iter()
            .map(|tab| tab.unwrap_or_else(|| "Core".to_string()))
            .collect()
    }

    pub fn active_agent_tab_label(&self) -> &str {
        self.active_agent_role.as_deref().unwrap_or("Core")
    }

    pub fn active_agent_role_config(&self) -> Option<&AgentRoleConfig> {
        self.active_agent_role
            .as_deref()
            .and_then(|role_id| self.agent_config.agent_role(role_id))
    }

    pub fn cycle_agent_role(&mut self, reverse: bool) -> bool {
        let tabs = self.agent_tabs();
        if tabs.len() <= 1 {
            return false;
        }

        let current_index = tabs
            .iter()
            .position(|tab| tab.as_ref() == self.active_agent_role.as_ref())
            .unwrap_or(0);
        let next_index = if reverse {
            (current_index + tabs.len() - 1) % tabs.len()
        } else {
            (current_index + 1) % tabs.len()
        };

        self.active_agent_role = tabs.get(next_index).cloned().flatten();
        true
    }

    fn agent_tabs(&self) -> Vec<Option<String>> {
        let order_mentions_core = self
            .agent_config
            .tui
            .agent_order
            .iter()
            .any(|tab| tab.trim().eq_ignore_ascii_case("core"));
        let mut tabs = Vec::new();
        let mut seen_core = false;
        let mut seen_roles = std::collections::BTreeSet::new();

        if !order_mentions_core {
            tabs.push(None);
            seen_core = true;
        }

        for configured_tab in &self.agent_config.tui.agent_order {
            let configured_tab = configured_tab.trim();
            if configured_tab.is_empty() {
                continue;
            }

            if configured_tab.eq_ignore_ascii_case("core") {
                if !seen_core {
                    tabs.push(None);
                    seen_core = true;
                }
                continue;
            }

            if let Some(role_id) = self
                .agent_config
                .agent
                .keys()
                .find(|role_id| role_id.eq_ignore_ascii_case(configured_tab))
                .cloned()
            {
                if seen_roles.insert(role_id.clone()) {
                    tabs.push(Some(role_id));
                }
            }
        }

        for role_id in self.agent_config.agent_role_ids() {
            if seen_roles.insert(role_id.clone()) {
                tabs.push(Some(role_id));
            }
        }

        if !seen_core {
            tabs.push(None);
        }

        tabs
    }

    pub fn cycle_reasoning_effort(&mut self) {
        self.reasoning_effort = self.reasoning_effort.next();
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

fn parse_tool_target_file_path(tool: &str, args_preview: &str) -> Option<String> {
    match tool {
        "file_edit" | "file_write" => {
            let value: serde_json::Value = serde_json::from_str(args_preview).ok()?;
            value.get("file_path")?.as_str().map(ToOwned::to_owned)
        }
        _ => None,
    }
}

pub(crate) fn file_change_delta_from_tool_args(
    tool: &str,
    args_preview: &str,
) -> Option<crate::chat::FileChangeDelta> {
    let value: serde_json::Value = serde_json::from_str(args_preview).ok()?;
    let file_path = value.get("file_path")?.as_str()?.to_string();
    let (additions, deletions) = match tool {
        "file_edit" => {
            let old_string = value.get("old_string")?.as_str()?;
            let new_string = value.get("new_string")?.as_str()?;
            line_change_counts(old_string, new_string)
        }
        "file_write" => {
            let content = value.get("content")?.as_str()?;
            (text_line_count(content), 0)
        }
        _ => return None,
    };

    if additions == 0 && deletions == 0 {
        return None;
    }

    Some(crate::chat::FileChangeDelta {
        file_path,
        additions,
        deletions,
    })
}

fn resolve_workspace_file_path(workspace: &Path, file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn read_file_if_exists(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => None,
    }
}

fn file_content_delta(
    file_path: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> crate::chat::FileChangeDelta {
    let (additions, deletions) = match (before, after) {
        (Some(before), Some(after)) => line_change_counts(before, after),
        (None, Some(after)) => (text_line_count(after), 0),
        (Some(before), None) => (0, text_line_count(before)),
        (None, None) => (0, 0),
    };

    crate::chat::FileChangeDelta {
        file_path: file_path.to_string(),
        additions,
        deletions,
    }
}

fn line_change_counts(before: &str, after: &str) -> (u32, u32) {
    if before == after {
        return (0, 0);
    }

    let before_lines = text_lines(before);
    let after_lines = text_lines(after);
    if before_lines.is_empty() {
        return (after_lines.len() as u32, 0);
    }
    if after_lines.is_empty() {
        return (0, before_lines.len() as u32);
    }

    let cell_count = before_lines.len().saturating_mul(after_lines.len());
    let common = if cell_count > 20_000 {
        coarse_common_line_count(&before_lines, &after_lines)
    } else {
        lcs_line_count(&before_lines, &after_lines)
    };

    (
        after_lines.len().saturating_sub(common) as u32,
        before_lines.len().saturating_sub(common) as u32,
    )
}

fn lcs_line_count(before_lines: &[&str], after_lines: &[&str]) -> usize {
    let mut previous = vec![0usize; after_lines.len() + 1];
    let mut current = vec![0usize; after_lines.len() + 1];
    for before_line in before_lines {
        for (after_index, after_line) in after_lines.iter().enumerate() {
            current[after_index + 1] = if before_line == after_line {
                previous[after_index] + 1
            } else {
                current[after_index].max(previous[after_index + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(0);
    }
    previous[after_lines.len()]
}

fn coarse_common_line_count(before_lines: &[&str], after_lines: &[&str]) -> usize {
    let mut prefix = 0usize;
    while prefix < before_lines.len().min(after_lines.len())
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < before_lines.len().saturating_sub(prefix)
        && suffix < after_lines.len().saturating_sub(prefix)
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    prefix + suffix
}

fn text_line_count(text: &str) -> u32 {
    text_lines(text).len() as u32
}

fn text_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.lines().collect()
    }
}

pub(crate) fn build_chat_state(config: &Config) -> ChatState {
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
    if let Some(context_window) = crate::config::resolve_context_window(config) {
        status_panel.set_context_window(context_window as u64);
    }
    if !config.llm.provider.trim().is_empty() && !config.llm.model.trim().is_empty() {
        status_panel.set_provider(&config.llm.provider, &config.llm.model);
    }
    status_panel.set_backend(sandbox_display_name(&config.operation_backend));
    status_panel
}

pub(crate) fn current_sandbox_id(config: &Config) -> &'static str {
    let Some(backend) = config.operation_backend.as_ref() else {
        return "local";
    };
    if backend.kind != "local" {
        return "local";
    }
    let Some(isolation) = backend.options.get("isolation") else {
        return "local";
    };
    match isolation.get("kind").and_then(|value| value.as_str()) {
        Some("macos_seatbelt") => "seatbelt",
        Some("linux_bubblewrap") => "bubblewrap",
        _ => "local",
    }
}

pub(crate) fn sandbox_display_name(backend: &Option<GatewayBackendConfig>) -> &'static str {
    let Some(backend) = backend.as_ref() else {
        return "Local";
    };
    if backend.kind != "local" {
        return "Local";
    }
    match backend
        .options
        .get("isolation")
        .and_then(|value| value.get("kind"))
        .and_then(|value| value.as_str())
    {
        Some("macos_seatbelt") => "Seatbelt",
        Some("linux_bubblewrap") => "Bubblewrap",
        _ => "Local",
    }
}

pub(crate) fn sandbox_backend_config(
    id: &str,
    current: &Option<GatewayBackendConfig>,
) -> Option<GatewayBackendConfig> {
    let mut options = current
        .as_ref()
        .filter(|backend| backend.kind == "local")
        .map(|backend| backend.options.clone())
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));

    if !options.is_object() {
        options = serde_json::Value::Object(serde_json::Map::new());
    }
    let object = options.as_object_mut()?;

    match id {
        "seatbelt" => {
            object.insert(
                "isolation".to_string(),
                serde_json::json!({
                    "kind": "macos_seatbelt"
                }),
            );
            Some(GatewayBackendConfig::new("local", options))
        }
        "bubblewrap" => {
            object.insert(
                "isolation".to_string(),
                serde_json::json!({
                    "kind": "linux_bubblewrap"
                }),
            );
            Some(GatewayBackendConfig::new("local", options))
        }
        _ => {
            object.remove("isolation");
            if object.is_empty() {
                None
            } else {
                Some(GatewayBackendConfig::new("local", options))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        current_sandbox_id, sandbox_backend_config,
        sandbox_display_name, ApiKeyDialogState, AppState, RuntimeStatusLight,
    };
    use crate::config::{AgentRoleConfig, Config};
    use crate::gateway::backend::GatewayBackendConfig;
    use crate::input::Input;
    use crate::interaction_prompt::{PromptChoice, PromptRequest};
    use agent_types::ReasoningEffort;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn runtime_status_light_is_idle_by_default() {
        let state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        assert_eq!(state.runtime_status_light(), RuntimeStatusLight::Idle);
    }

    #[test]
    fn agent_tab_labels_allow_core_in_configured_order() {
        let mut config = Config::default();
        for role_id in ["baize", "xuanyuan", "plan"] {
            config
                .agent
                .insert(role_id.to_string(), AgentRoleConfig::default());
        }
        config.tui.agent_order = vec![
            "xuanyuan".to_string(),
            "Core".to_string(),
            "baize".to_string(),
        ];
        let mut state = AppState::new_with_config(
            &config,
            PathBuf::from("config.toml"),
            PathBuf::from("."),
        )
        .expect("app state should initialize");

        assert_eq!(
            state.agent_tab_labels(),
            vec!["xuanyuan", "Core", "baize", "plan"]
        );

        assert!(state.cycle_agent_role(false));
        assert_eq!(state.active_agent_tab_label(), "xuanyuan");
        assert!(state.cycle_agent_role(false));
        assert_eq!(state.active_agent_tab_label(), "Core");
    }

    #[test]
    fn sandbox_backend_config_preserves_local_options_when_enabling_seatbelt() {
        let current = Some(GatewayBackendConfig::new(
            "local",
            json!({"default_shell": "/bin/zsh"}),
        ));

        let updated = sandbox_backend_config("seatbelt", &current).expect("backend");

        assert_eq!(updated.kind, "local");
        assert_eq!(updated.options["default_shell"], "/bin/zsh");
        assert_eq!(updated.options["isolation"]["kind"], "macos_seatbelt");
    }

    #[test]
    fn sandbox_backend_config_removes_only_isolation_when_switching_local() {
        let current = Some(GatewayBackendConfig::new(
            "local",
            json!({
                "default_shell": "/bin/zsh",
                "isolation": {"kind": "macos_seatbelt"}
            }),
        ));

        let updated = sandbox_backend_config("local", &current).expect("backend");

        assert_eq!(updated.options["default_shell"], "/bin/zsh");
        assert!(updated.options.get("isolation").is_none());
    }

    #[test]
    fn sandbox_backend_config_preserves_local_options_when_enabling_bubblewrap() {
        let current = Some(GatewayBackendConfig::new(
            "local",
            json!({"default_shell": "/bin/bash"}),
        ));

        let updated = sandbox_backend_config("bubblewrap", &current).expect("backend");

        assert_eq!(updated.kind, "local");
        assert_eq!(updated.options["default_shell"], "/bin/bash");
        assert_eq!(updated.options["isolation"]["kind"], "linux_bubblewrap");
    }

    #[test]
    fn sandbox_helpers_recognize_bubblewrap() {
        let mut config = Config::default();
        config.operation_backend = Some(GatewayBackendConfig::new(
            "local",
            json!({"isolation": {"kind": "linux_bubblewrap"}}),
        ));

        assert_eq!(current_sandbox_id(&config), "bubblewrap");
        assert_eq!(
            sandbox_display_name(&config.operation_backend),
            "Bubblewrap"
        );
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
    fn cycle_reasoning_effort_rotates_off_high_max() {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");

        assert_eq!(state.reasoning_effort, ReasoningEffort::Off);
        state.cycle_reasoning_effort();
        assert_eq!(state.reasoning_effort, ReasoningEffort::High);
        state.cycle_reasoning_effort();
        assert_eq!(state.reasoning_effort, ReasoningEffort::Max);
        state.cycle_reasoning_effort();
        assert_eq!(state.reasoning_effort, ReasoningEffort::Off);
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

    #[test]
    fn session_file_change_uses_content_baseline_for_existing_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let file = workspace.join("README.md");
        fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").expect("baseline");

        let mut state = AppState::new(PathBuf::from("config.toml"), workspace)
            .expect("app state should initialize");
        state.capture_tool_file_baseline("call-1", "file_edit", r#"{"file_path":"README.md"}"#);

        fs::write(&file, "one\ntwo\nTHREE\nfour\nfive\n").expect("modified");
        state.reconcile_tool_file_change_from_baseline("call-1", None);

        let stats = state
            .session_file_changes
            .get("README.md")
            .expect("session stats should be tracked");
        assert_eq!(stats.additions, 1);
        assert_eq!(stats.deletions, 1);
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
