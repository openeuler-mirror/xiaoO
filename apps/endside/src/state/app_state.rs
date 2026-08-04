use agent_types::ReasoningEffort;
use anyhow::Result;
use ratatui::{layout::Rect, text::Line};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use xiaoo_shared::session_diff::SessionDiffTracker;

use crate::backend::GatewayBackendConfig;
use crate::chat::{default_provider_list, merge_config_provider, ChatState, TodoMessageState};
use crate::config::{AgentRoleConfig, Config};
use crate::input::Input;
use crate::interaction_prompt::{InteractionPromptState, PromptRequest};
use crate::provider_dialog::ProviderDialog;
use crate::render::markdown::MarkdownIncrementalState;
use crate::selection::TranscriptSelection;
use crate::services::command_loader::{load_external_commands, ExternalCommand};
use crate::services::input_history::load_input_history;
use crate::slash_complete::{apply_slash_pick, candidates_for_prefix, slash_typed_prefix};
use crate::status_panel::StatusPanel;
use crate::theme::Theme;

#[derive(PartialEq)]
pub enum InputMode {
    Editing,
    ProviderSelection,
    SandboxSelection,
    RemoteSessionSelection,
    SessionSnapshotSelection,
    InteractionPrompt,
    TurnDelete,
    CronManagement,
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

#[derive(Debug, Clone)]
pub struct SubagentOpenRegion {
    pub agent_id: String,
    pub rect: Rect,
}

#[derive(Clone)]
pub struct SubagentOpenTarget {
    pub agent_id: String,
    pub row_offset: usize,
}

#[derive(Clone)]
pub struct CachedMessageRender {
    pub width: u16,
    pub lines: Vec<Line<'static>>,
    pub wrapped_lines: Option<Vec<Vec<Line<'static>>>>,
    pub tool_toggle_row_offset: Option<usize>,
    pub subagent_open_target: Option<SubagentOpenTarget>,
    /// `Some(n)` for the active streaming assistant message rendered via the
    /// incremental markdown path: `lines` / `wrapped_lines` contain the
    /// SUFFIX only (the frozen prefix of `n` logical lines is moved from the
    /// previous tick's block by `build_transcript_cache`). `None` for every
    /// other message — `lines` is the complete output.
    pub frozen_prefix_line_count: Option<usize>,
}

/// Per-message visual render block stored inside [`TranscriptRenderCache`].
///
/// Non-dirty messages move their `lines` / `visual_lines` from the previous
/// tick's cache into the new one (zero `Line` clone); only dirty messages
/// re-wrap. `logical_to_visual_offset[i]` is the local visual row where
/// logical line `i` begins within this block — kept so the flat
/// `logical_line_visual_starts` index can be rebuilt without re-walking the
/// (moved) `visual_lines`.
#[derive(Clone)]
pub struct MessageVisualBlock {
    pub message_index: usize,
    pub start_visual_row: usize,
    pub logical_line_start: usize,
    pub lines: Vec<Line<'static>>,
    pub visual_lines: Vec<Line<'static>>,
    pub logical_to_visual_offset: Vec<usize>,
    pub tool_toggle_row_offset: Option<usize>,
    pub subagent_open_target: Option<SubagentOpenTarget>,
}

#[derive(Clone)]
pub struct TranscriptRenderCache {
    pub message_blocks: Vec<MessageVisualBlock>,
    /// Flat index: visual row where each logical line begins (global).
    /// Rebuilt each tick from `message_blocks` (O(n_logical), no `Line` clone).
    pub logical_line_visual_starts: Vec<usize>,
    /// Flat per-logical-line plain text (mouse / selection copy source).
    pub line_texts: Vec<String>,
    /// Flat per-logical-line "is role/tool header" flag.
    pub line_is_header: Vec<bool>,
    /// Flat per-visual-line background colour (`paint_visible_line_backgrounds`).
    pub visual_line_backgrounds: Vec<Option<ratatui::style::Color>>,
    pub total_lines: usize,
}

impl TranscriptRenderCache {
    /// Number of logical lines across all blocks.
    pub fn logical_line_count(&self) -> usize {
        self.line_texts.len()
    }

    /// Borrow a single visual line by global visual row index.
    pub fn visual_line(&self, visual_row: usize) -> Option<&Line<'static>> {
        if visual_row >= self.total_lines {
            return None;
        }
        let block_idx = self
            .message_blocks
            .partition_point(|b| b.start_visual_row <= visual_row)
            .saturating_sub(1);
        let block = self.message_blocks.get(block_idx)?;
        let local = visual_row - block.start_visual_row;
        block.visual_lines.get(local)
    }

    /// Borrow a single logical line by global logical line index.
    pub fn logical_line(&self, logical_idx: usize) -> Option<&Line<'static>> {
        let block_idx = self
            .message_blocks
            .partition_point(|b| b.logical_line_start <= logical_idx)
            .saturating_sub(1);
        let block = self.message_blocks.get(block_idx)?;
        let local = logical_idx - block.logical_line_start;
        block.lines.get(local)
    }

    /// Collect the visible visual-line window `[scroll_offset, visual_end)`
    /// by cloning only the relevant slices from `message_blocks`. This is the
    /// sole remaining `Line` clone site per frame, bounded by `inner_height`
    /// instead of the full transcript.
    pub fn collect_visible_visual_lines(
        &self,
        scroll_offset: usize,
        visual_end: usize,
    ) -> Vec<Line<'static>> {
        if scroll_offset >= visual_end || scroll_offset >= self.total_lines {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(visual_end.saturating_sub(scroll_offset));
        let mut row = scroll_offset;
        while row < visual_end && row < self.total_lines {
            let block_idx = self
                .message_blocks
                .partition_point(|b| b.start_visual_row <= row)
                .saturating_sub(1);
            let Some(block) = self.message_blocks.get(block_idx) else {
                break;
            };
            let local = row - block.start_visual_row;
            let block_remaining = block.visual_lines.len().saturating_sub(local);
            let take = block_remaining.min(visual_end - row);
            if take == 0 {
                break;
            }
            out.extend(block.visual_lines[local..local + take].iter().cloned());
            row += take;
        }
        out
    }
}

#[derive(Default)]
pub struct RenderState {
    pub messages_area: Option<Rect>,
    pub theme_toggle_area: Option<Rect>,
    pub api_key_toggle_area: Option<Rect>,
    /// Per-message last-applied `render_revision`. `None` means "not yet
    /// rendered" (dirty). Replaces the former `message_renders:
    /// Vec<Option<CachedMessageRender>>` — we now keep only the revision
    /// fingerprint instead of the full render tree, and the render itself
    /// lives inside `TranscriptRenderCache::message_blocks`.
    pub message_render_revisions: Vec<Option<u64>>,
    /// Incremental markdown render state for the single active streaming
    /// message. Only one message streams at a time; invalidated on width /
    /// theme / transcript changes (see `render_chat`). `None` for every
    /// non-streaming message.
    pub incremental_markdown: Option<MarkdownIncrementalState>,
    /// Message index that `incremental_markdown` was produced for. When the
    /// active streaming index moves (stream settles / switches), the state
    /// is cleared so a stale cache is never reused.
    pub incremental_markdown_index: Option<usize>,
    /// Width used to build the current `transcript_cache`. A change forces
    /// every message dirty (re-wrap).
    pub last_render_width: Option<u16>,
    /// Theme used to build the current `transcript_cache`. A change forces
    /// every message dirty (re-style).
    pub last_render_theme: Option<Theme>,
    pub transcript_cache: Option<TranscriptRenderCache>,
    pub tool_toggle_regions: Vec<ToolToggleRegion>,
    pub subagent_open_regions: Vec<SubagentOpenRegion>,
    pub slash_popup_inner: Option<Rect>,
    pub interaction_prompt_list_area: Option<Rect>,
    pub interaction_prompt_supplement_area: Option<Rect>,
    /// Index of the first visible agent tab in the header.
    /// Used for horizontal scrolling when there are many agent tabs.
    pub first_visible_agent_tab: usize,
    pub active_transcript_key: Option<String>,
    /// Cached terminal area for layout reuse across ticks.
    /// When `frame.area()` matches `cached_area`, layout splits are skipped.
    pub cached_area: Option<Rect>,
    /// Cached vertical layout chunks (header, body, input, status).
    pub cached_chunks: Vec<Rect>,
    /// Cached body chunks (chat, sidebar).
    pub cached_body_chunks: Vec<Rect>,
    /// Cached sidebar visibility computed from last layout.
    ///
    /// Depends on `AppState::plan_state.is_some()` (at terminal widths
    /// 60..=71); a Some<->None transition must invalidate `cached_area` so the
    /// body split is recomputed. See `apply_todo_snapshot`.
    pub cached_show_sidebar: bool,
}

#[derive(Default)]
pub struct SlashState {
    pub selected: usize,
    pub dismissed_prefix: Option<String>,
}

pub use xiaoo_shared::session_diff::{SessionFileChangeEntry, SessionFileChangeStats};

pub struct AppState {
    pub theme: Theme,
    pub chat_state: ChatState,
    pub status_panel: StatusPanel,
    pub input_mode: InputMode,
    pub should_quit: bool,
    /// Set when the user quits via an interrupt (Ctrl+C / SIGINT / SIGTERM)
    /// so that `App::run` can auto-save the session before shutting down.
    pub quit_via_interrupt: bool,
    pub provider_dialog: Option<ProviderDialog>,
    pub sandbox_dialog: Option<SandboxDialog>,
    pub remote_session_dialog: Option<crate::remote_sessions_service::RemoteSessionDialog>,
    pub session_snapshot_dialog: Option<crate::session_snapshot_service::SessionSnapshotDialog>,
    pub delete_dialog: Option<crate::services::turn_delete::DeleteDialog>,
    pub cron_dialog: Option<crate::cron_dialog::CronDialog>,
    pub api_key_dialog: Option<ApiKeyDialogState>,
    pub agent_config: Config,
    pub active_agent_role: Option<String>,
    pub reasoning_effort: ReasoningEffort,
    pub config_path: PathBuf,
    pub workspace: PathBuf,
    pub session_messages: Vec<llm_client::ChatMessage>,
    pub plan_state: Option<TodoMessageState>,
    pub session_id: String,
    /// Per-process ephemeral UUID sent with every remote RPC; used by the
    /// daemon's attach-lease table to enforce single-writer per session.
    pub client_id: String,
    /// Set when the daemon reports this session has been taken over by
    /// another `client_id`; the TUI then refuses further submissions.
    pub session_taken_over: bool,
    pub current_snapshot_context: Option<crate::session_snapshot_service::SnapshotContext>,
    pub slash: SlashState,
    pub interaction_prompt: Option<InteractionPromptState>,
    pub render_state: RenderState,
    /// Active text selection in the transcript area, if any.
    pub transcript_selection: Option<TranscriptSelection>,
    /// Set when text is copied to clipboard; drives the toast notification.
    pub copy_notice: Option<Instant>,
    pub external_commands: Vec<ExternalCommand>,
    pub diff_tracker: SessionDiffTracker,
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
            quit_via_interrupt: false,
            provider_dialog: None,
            sandbox_dialog: None,
            remote_session_dialog: None,
            session_snapshot_dialog: None,
            delete_dialog: None,
            cron_dialog: None,
            api_key_dialog: None,
            agent_config: Config::default(),
            active_agent_role: None,
            reasoning_effort: Config::default().llm.reasoning_effort,
            config_path,
            workspace: workspace.clone(),
            session_messages: Vec::new(),
            plan_state: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            client_id: uuid::Uuid::new_v4().to_string(),
            session_taken_over: false,
            current_snapshot_context: None,
            slash: SlashState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            transcript_selection: None,
            copy_notice: None,
            external_commands: load_external_commands(),
            diff_tracker: SessionDiffTracker::new(workspace),
        })
    }

    pub fn new_with_config(
        config: &Config,
        config_path: PathBuf,
        workspace: PathBuf,
    ) -> Result<Self, anyhow::Error> {
        let input_history = load_input_history().unwrap_or_else(|error| {
            tracing::warn!("failed to load input history: {error:#}");
            Vec::new()
        });
        let mut chat_state = build_chat_state(config);
        chat_state.set_input_history(input_history);

        Ok(Self {
            theme: Theme::default(),
            chat_state,
            status_panel: build_status_panel(config),
            input_mode: InputMode::Editing,
            should_quit: false,
            quit_via_interrupt: false,
            provider_dialog: None,
            sandbox_dialog: None,
            remote_session_dialog: None,
            session_snapshot_dialog: None,
            delete_dialog: None,
            cron_dialog: None,
            api_key_dialog: None,
            agent_config: config.clone(),
            active_agent_role: None,
            reasoning_effort: config.llm.reasoning_effort,
            config_path,
            workspace: workspace.clone(),
            session_messages: Vec::new(),
            plan_state: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            // Fresh per-process UUID: sharing a persisted id would let two
            // TUIs refresh each other's lease and bypass single-writer.
            client_id: uuid::Uuid::new_v4().to_string(),
            session_taken_over: false,
            current_snapshot_context: None,
            slash: SlashState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            external_commands: load_external_commands(),
            transcript_selection: None,
            copy_notice: None,
            diff_tracker: SessionDiffTracker::new(workspace),
        })
    }

    pub fn reset_for_new_session(&mut self) {
        let input_history = self.chat_state.input_history.clone();
        self.chat_state = build_chat_state(&self.agent_config);
        self.chat_state.set_input_history(input_history);
        self.status_panel = build_status_panel(&self.agent_config);
        self.status_panel.set_workspace(&self.workspace);
        self.input_mode = InputMode::Editing;
        self.provider_dialog = None;
        self.sandbox_dialog = None;
        self.remote_session_dialog = None;
        self.session_snapshot_dialog = None;
        self.delete_dialog = None;
        self.api_key_dialog = None;
        self.session_messages.clear();
        self.plan_state = None;
        self.session_id = uuid::Uuid::new_v4().to_string();
        self.session_taken_over = false;
        self.current_snapshot_context = None;
        self.slash = SlashState::default();
        self.reasoning_effort = ReasoningEffort::default();
        self.interaction_prompt = None;
        self.render_state = RenderState::default();
        self.transcript_selection = None;
        self.copy_notice = None;
        self.external_commands = load_external_commands();
        self.diff_tracker.clear();
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

    pub fn active_transcript_key(&self) -> String {
        self.chat_state
            .active_subagent_id()
            .map(|agent_id| format!("subagent:{agent_id}"))
            .unwrap_or_else(|| "main".to_string())
    }

    pub fn is_subagent_view_active(&self) -> bool {
        self.chat_state.is_subagent_view_active()
    }

    pub fn active_transcript_has_tool_cards(&self) -> bool {
        if let Some(agent_id) = self.chat_state.active_subagent_id() {
            return self
                .chat_state
                .subagent_lanes
                .get(agent_id)
                .map(|lane| {
                    lane.messages
                        .iter()
                        .any(|message| message.tool_state.is_some())
                })
                .unwrap_or(false);
        }
        self.chat_state
            .messages
            .iter()
            .any(|message| message.tool_state.is_some())
    }

    pub fn active_subagent_title(&self) -> Option<String> {
        let agent_id = self.chat_state.active_subagent_id()?;
        let lane = self.chat_state.subagent_lanes.get(agent_id)?;
        let mut title = if lane.title.trim().is_empty() {
            format!("Subagent {}", short_agent_id(&lane.agent_id))
        } else {
            lane.title.clone()
        };
        if lane.is_running {
            title.push_str(" (running)");
        }
        Some(title)
    }

    pub fn active_subagent_readonly_text(&self) -> String {
        let Some(agent_id) = self.chat_state.active_subagent_id() else {
            return String::new();
        };
        let Some(lane) = self.chat_state.subagent_lanes.get(agent_id) else {
            return format!("Subagent {}", short_agent_id(agent_id));
        };
        let mut parts = Vec::new();
        if !lane.description.trim().is_empty() {
            parts.push(lane.description.trim().to_string());
        }
        if !lane.task_goal.trim().is_empty() {
            parts.push(lane.task_goal.trim().to_string());
        }
        if parts.is_empty() {
            format!("Subagent {}", short_agent_id(&lane.agent_id))
        } else {
            parts.join("\n")
        }
    }

    pub fn enter_subagent_view(&mut self, agent_id: &str) -> bool {
        let entered = self.chat_state.enter_subagent_view(agent_id);
        if entered {
            self.invalidate_transcript_render_cache();
            self.transcript_selection = None;
            self.chat_state.input.clear_selection();
        }
        entered
    }

    pub fn leave_subagent_view(&mut self) -> bool {
        let left = self.chat_state.leave_subagent_view();
        if left {
            self.invalidate_transcript_render_cache();
            self.transcript_selection = None;
        }
        left
    }

    pub fn invalidate_transcript_render_cache(&mut self) {
        self.render_state.message_render_revisions.clear();
        self.render_state.last_render_width = None;
        self.render_state.last_render_theme = None;
        self.render_state.transcript_cache = None;
        self.render_state.tool_toggle_regions.clear();
        self.render_state.subagent_open_regions.clear();
        self.render_state.active_transcript_key = None;
    }

    pub fn active_transcript_scroll_up(&mut self) {
        if let Some(agent_id) = self.chat_state.active_subagent_id().map(ToOwned::to_owned) {
            if let Some(lane) = self.chat_state.subagent_lanes.get_mut(&agent_id) {
                lane.scroll_up();
            }
        } else {
            self.chat_state.scroll_up();
        }
    }

    pub fn active_transcript_scroll_down(&mut self) {
        if let Some(agent_id) = self.chat_state.active_subagent_id().map(ToOwned::to_owned) {
            if let Some(lane) = self.chat_state.subagent_lanes.get_mut(&agent_id) {
                lane.scroll_down();
            }
        } else {
            self.chat_state.scroll_down();
        }
    }

    pub fn active_transcript_scroll_offset(&self) -> usize {
        self.chat_state
            .active_subagent_id()
            .and_then(|agent_id| self.chat_state.subagent_lanes.get(agent_id))
            .map(|lane| lane.scroll_offset)
            .unwrap_or(self.chat_state.scroll_offset)
    }

    pub fn active_transcript_max_scroll_offset(&self) -> usize {
        self.chat_state
            .active_subagent_id()
            .and_then(|agent_id| self.chat_state.subagent_lanes.get(agent_id))
            .map(|lane| lane.max_scroll_offset())
            .unwrap_or_else(|| self.chat_state.max_scroll_offset())
    }

    pub fn set_active_transcript_scroll_offset(&mut self, line_offset: usize) {
        if let Some(agent_id) = self.chat_state.active_subagent_id().map(ToOwned::to_owned) {
            if let Some(lane) = self.chat_state.subagent_lanes.get_mut(&agent_id) {
                lane.set_scroll_offset(line_offset);
            }
        } else {
            self.chat_state.set_scroll_offset(line_offset);
        }
    }

    pub fn active_transcript_scrollbar_dragging(&self) -> bool {
        self.chat_state
            .active_subagent_id()
            .and_then(|agent_id| self.chat_state.subagent_lanes.get(agent_id))
            .map(|lane| lane.scrollbar_dragging)
            .unwrap_or(self.chat_state.scrollbar_dragging)
    }

    pub fn set_active_transcript_scrollbar_dragging(&mut self, dragging: bool) {
        if let Some(agent_id) = self.chat_state.active_subagent_id().map(ToOwned::to_owned) {
            if let Some(lane) = self.chat_state.subagent_lanes.get_mut(&agent_id) {
                lane.scrollbar_dragging = dragging;
            }
        } else {
            self.chat_state.scrollbar_dragging = dragging;
        }
    }

    pub fn clear_tool_file_baselines(&mut self) {
        self.diff_tracker.clear_tool_file_baselines();
    }

    /// High-level entry: tool transitioned to Running.
    pub fn on_tool_running(&mut self, call_id: &str, tool: &str, args_preview: &str) {
        self.diff_tracker
            .on_tool_running(call_id, tool, args_preview);
    }

    /// High-level entry: tool transitioned to Completed. Returns the computed
    /// delta (used in remote mode to forward to the TUI; local callers may
    /// discard it).
    pub fn on_tool_completed(
        &mut self,
        call_id: &str,
        tool: &str,
        args_preview: &str,
        file_change: Option<crate::chat::FileChangeDelta>,
    ) -> Option<crate::chat::FileChangeDelta> {
        self.diff_tracker
            .on_tool_completed(call_id, tool, args_preview, file_change.map(Into::into))
            .map(Into::into)
    }

    /// High-level entry: tool transitioned to Failed.
    pub fn on_tool_failed(
        &mut self,
        call_id: &str,
        file_change: Option<crate::chat::FileChangeDelta>,
    ) -> Option<crate::chat::FileChangeDelta> {
        self.diff_tracker
            .on_tool_failed(call_id, file_change.map(Into::into))
            .map(Into::into)
    }

    /// Remote-mode entry: directly apply a delta precomputed by the daemon.
    pub fn apply_remote_delta(&mut self, call_id: &str, delta: crate::chat::FileChangeDelta) {
        self.diff_tracker.apply_remote_delta(call_id, delta.into());
    }

    /// Replace the tracker's session changes (used by snapshot restore).
    pub fn restore_session_file_changes(
        &mut self,
        snapshot: std::collections::BTreeMap<String, SessionFileChangeStats>,
    ) {
        self.diff_tracker.restore(snapshot);
    }

    pub fn session_file_changes(
        &self,
    ) -> &std::collections::BTreeMap<String, SessionFileChangeStats> {
        self.diff_tracker.session_file_changes()
    }

    pub fn sorted_session_file_changes(&self) -> Vec<SessionFileChangeEntry> {
        self.diff_tracker.sorted_session_file_changes()
    }

    /// Synchronize the diff tracker's workspace with `self.workspace`.
    /// Must be called whenever `self.workspace` is mutated externally so
    /// that [`Self::display_file_path`] strips prefixes against the active
    /// workspace rather than a stale one captured at construction time.
    pub fn sync_diff_tracker_workspace(&mut self) {
        self.diff_tracker.set_workspace(self.workspace.clone());
    }

    pub fn display_file_path(&self, file_path: &str) -> String {
        self.diff_tracker.display_file_path(file_path)
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
        let cache = self.render_state.transcript_cache.as_ref()?;
        let (start_line, start_col, end_line, end_col) = sel.normalised();
        let lines = &cache.line_texts;

        if start_line >= lines.len() {
            return None;
        }

        let mut segments: Vec<String> = Vec::new();
        for line_idx in start_line..=end_line.min(lines.len().saturating_sub(1)) {
            // Skip role/tool/planner header lines (▎ Role  HH:MM:SS).
            if cache.line_is_header.get(line_idx).copied().unwrap_or(false) {
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
        if self.is_subagent_view_active() {
            return false;
        }
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

fn short_agent_id(agent_id: &str) -> String {
    let trimmed = agent_id.trim();
    if trimmed.chars().count() <= 8 {
        trimmed.to_string()
    } else {
        trimmed.chars().take(8).collect::<String>()
    }
}

pub(crate) fn file_change_delta_from_tool_args(
    tool: &str,
    args_preview: &str,
) -> Option<crate::chat::FileChangeDelta> {
    xiaoo_shared::session_diff::file_change_delta_from_tool_args(tool, args_preview).map(Into::into)
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
        current_sandbox_id, sandbox_backend_config, sandbox_display_name, ApiKeyDialogState,
        AppState, RuntimeStatusLight,
    };
    use crate::backend::GatewayBackendConfig;
    use crate::config::{AgentRoleConfig, Config};
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
        let mut state =
            AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
                .expect("app state should initialize");

        assert_eq!(
            state.agent_tab_labels(),
            vec!["xuanyuan", "Core", "baize", "plan"]
        );

        assert!(state.cycle_agent_role(false));
        assert_eq!(state.active_agent_tab_label(), "baize");
        assert!(state.cycle_agent_role(false));
        assert_eq!(state.active_agent_tab_label(), "plan");
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
        state.on_tool_running("call-1", "file_edit", r#"{"file_path":"README.md"}"#);

        fs::write(&file, "one\ntwo\nTHREE\nfour\nfive\n").expect("modified");
        state.on_tool_completed("call-1", "file_edit", r#"{"file_path":"README.md"}"#, None);

        let stats = state
            .session_file_changes()
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
            is_secret: false,
            default_index: Some(0),
        }
    }
}
