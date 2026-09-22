use anyhow::Result;
use ratatui::{layout::Rect, text::Line, widgets::ScrollbarState};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use xiaoo_api::chat::ReasoningEffort;
use xiaoo_shared::session_diff::SessionDiffTracker;

use crate::backend::GatewayBackendConfig;
use crate::chat::{
    default_provider_list, merge_config_provider, ChatState, Message, TodoMessageState,
};
use crate::config::{AgentRoleConfig, Config};
use crate::input::file_mention::{
    file_mention_candidates, file_mention_token, FileMentionCandidate, FileMentionToken,
    FILE_MENTION_MAX_CANDIDATES,
};
use crate::input::mouse_to_line_col;
use crate::input::Input;
use crate::interaction_prompt::{InteractionPromptState, PromptRequest};
use crate::provider_dialog::ProviderDialog;
use crate::provider_service::ClipboardOutcome;
use crate::render::markdown::{MarkdownIncrementalState, WideTableRegion};
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
            options.push(SandboxOption {
                id: "dynsandbox",
                name: "Dyn-Sandbox",
                description: "Linux dynsandbox + local file policy。",
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

/// A wide markdown table that is (at least partially) visible this frame.
/// Derived in `render_chat` from each block's `wide_tables` metadata and used
/// for mouse hit-testing (Shift+wheel) and as the keyboard scroll target.
#[derive(Debug, Clone, Copy)]
pub struct WideTableScrollRegion {
    /// Owning message index within the transcript currently on screen (main
    /// chat or the active subagent lane). The window itself lives on
    /// `Message::table_horiz_offset`.
    pub message_index: usize,
    /// On-screen rectangle of the visible portion of the table.
    pub rect: Rect,
    /// Render viewport width (terminal columns) the window was clipped to.
    /// Used for the horizontal scroll step so it matches the window actually
    /// rendered instead of the outer message-area width.
    pub viewport_width: usize,
    /// Maximum `table_horiz_offset` (`natural_width - viewport_width`); a
    /// scroll clamps to `[0, max_offset]`.
    pub max_offset: usize,
}

#[derive(Debug, Clone)]
pub struct SubagentOpenRegion {
    pub agent_id: String,
    pub rect: Rect,
}

/// Scroll state for the sidebar "Plan" (current task list) panel.
///
/// Mirrors the line-based scroll bookkeeping of `SubagentLaneState`:
/// `total_lines` / `last_visible_height` are refreshed by every
/// `render_plan_panel` frame (the panel content is word-wrapped, so the
/// visual line count depends on the current sidebar width), and
/// `scroll_offset` is the number of wrapped visual lines skipped from the
/// top of the panel.
pub struct PlanPanelState {
    /// Line-based scroll: wrapped visual lines skipped from the panel top.
    pub scroll_offset: usize,
    pub scrollbar_state: ScrollbarState,
    /// True while the user is dragging the plan panel scrollbar thumb.
    pub scrollbar_dragging: bool,
    /// Total wrapped line count of the plan panel (updated each render).
    pub total_lines: usize,
    /// Inner height of the plan panel (updated each render).
    pub last_visible_height: usize,
}

impl Default for PlanPanelState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            scrollbar_dragging: false,
            total_lines: 0,
            last_visible_height: 0,
        }
    }
}

impl PlanPanelState {
    pub fn max_scroll_offset(&self) -> usize {
        self.total_lines
            .saturating_sub(self.last_visible_height)
            .min(self.total_lines)
    }

    pub fn sync_scrollbar_state(&mut self) {
        let max_scroll = self.max_scroll_offset();
        let scrollbar_content_length = max_scroll.saturating_add(1);
        self.scrollbar_state = self
            .scrollbar_state
            .content_length(scrollbar_content_length)
            .viewport_content_length(self.last_visible_height)
            .position(self.scroll_offset.min(max_scroll));
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.sync_scrollbar_state();
    }

    pub fn scroll_down(&mut self) {
        let max = self.max_scroll_offset();
        if self.scroll_offset < max {
            self.scroll_offset = (self.scroll_offset + 1).min(max);
        }
        self.sync_scrollbar_state();
    }

    pub fn set_scroll_offset(&mut self, line_offset: usize) {
        let max = self.max_scroll_offset();
        self.scroll_offset = line_offset.min(max);
        self.sync_scrollbar_state();
    }

    /// Reset scroll to the top (new plan snapshot / session switch).
    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
        self.scrollbar_dragging = false;
        self.sync_scrollbar_state();
    }
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
    /// Wide markdown tables (message-logical-line start, window metadata).
    /// Empty for messages without any wide table.
    pub wide_tables: Vec<WideTableRegion>,
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
    /// Wide markdown tables, `start_line` rebased to this block's logical
    /// lines. Survives zero-clone moves alongside `lines`/`visual_lines`.
    pub wide_tables: Vec<WideTableRegion>,
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
    /// Scrollbar track area of the sidebar Plan panel (outer x/width, inner
    /// y/height — same convention as `messages_area`). Used for mouse
    /// hit-testing (wheel scroll + thumb drag). `None` when no plan shows.
    pub plan_panel_area: Option<Rect>,
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
    /// Visible wide tables this frame (mouse hit-testing for horizontal
    /// scroll). Rebuilt every frame from `transcript_cache` blocks.
    pub wide_table_regions: Vec<WideTableScrollRegion>,
    /// Topmost visible wide table; the keyboard target for ←/→.
    pub wide_table_keyboard_target: Option<WideTableScrollRegion>,
    pub slash_popup_inner: Option<Rect>,
    pub file_mention_popup_inner: Option<Rect>,
    pub file_mention_view_start: usize,
    pub interaction_prompt_list_area: Option<Rect>,
    pub interaction_prompt_supplement_area: Option<Rect>,
    /// Index of the first visible agent tab in the header.
    /// Used for horizontal scrolling when there are many agent tabs.
    pub first_visible_agent_tab: usize,
    /// Inner (content) area of the input box, used for mouse drag-select.
    pub input_area: Option<Rect>,
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

#[derive(Default)]
pub struct FileMentionState {
    pub selected: usize,
    pub dismissed_prefix: Option<String>,
    pub cached_token: Option<FileMentionToken>,
    pub candidates: Vec<FileMentionCandidate>,
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
    pub session_messages: Vec<xiaoo_api::chat::ChatMessage>,
    pub plan_state: Option<TodoMessageState>,
    /// Scroll state for the sidebar Plan panel (current task list).
    pub plan_panel: PlanPanelState,
    pub session_id: String,
    /// Per-process ephemeral UUID sent with every remote RPC; used by the
    /// daemon's attach-lease table to enforce single-writer per session.
    pub client_id: String,
    /// Set when the daemon reports this session has been taken over by
    /// another `client_id`; the TUI then refuses further submissions.
    pub session_taken_over: bool,
    pub current_snapshot_context: Option<crate::session_snapshot_service::SnapshotContext>,
    pub slash: SlashState,
    pub file_mention: FileMentionState,
    pub interaction_prompt: Option<InteractionPromptState>,
    pub render_state: RenderState,
    /// Active text selection in the transcript area, if any.
    pub transcript_selection: Option<TranscriptSelection>,
    /// Set when text is copied to clipboard; drives the toast notification.
    pub copy_notice: Option<Instant>,
    /// Set when a copy attempt failed (no local clipboard tool and the
    /// terminal did not support OSC 52); drives an error toast.
    pub copy_error_notice: Option<Instant>,
    pub external_commands: Vec<ExternalCommand>,
    pub diff_tracker: SessionDiffTracker,
}

impl AppState {
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
            plan_panel: PlanPanelState::default(),
            session_id: uuid::Uuid::new_v4().to_string(),
            // Fresh per-process UUID: sharing a persisted id would let two
            // TUIs refresh each other's lease and bypass single-writer.
            client_id: uuid::Uuid::new_v4().to_string(),
            session_taken_over: false,
            current_snapshot_context: None,
            slash: SlashState::default(),
            file_mention: FileMentionState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            external_commands: load_external_commands(),
            transcript_selection: None,
            copy_notice: None,
            copy_error_notice: None,
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
        self.plan_panel = PlanPanelState::default();
        self.session_id = uuid::Uuid::new_v4().to_string();
        self.session_taken_over = false;
        self.current_snapshot_context = None;
        self.slash = SlashState::default();
        self.file_mention = FileMentionState::default();
        self.reasoning_effort = self.agent_config.llm.reasoning_effort;
        self.interaction_prompt = None;
        self.render_state = RenderState::default();
        self.transcript_selection = None;
        self.copy_notice = None;
        self.copy_error_notice = None;
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

    /// Mark that a copy attempt failed; shows the error toast for 2 s.
    pub fn set_copy_error_notice(&mut self) {
        self.copy_error_notice = Some(Instant::now());
    }

    /// Returns `true` while the copy-error toast should still be visible.
    pub fn copy_error_notice_active(&self) -> bool {
        self.copy_error_notice
            .map(|t| t.elapsed() < Duration::from_millis(2000))
            .unwrap_or(false)
    }

    /// Report the outcome of a clipboard copy attempt: shows the success
    /// toast only when delivery is confirmed (native clipboard tool) or the
    /// terminal is known to honour OSC 52. A fire-and-forget OSC 52 sequence
    /// into an unknown terminal (PuTTY, MobaXterm, FinalShell, Xshell …)
    /// shows the error toast instead — previously the UI claimed
    /// "Copied to clipboard" while the terminal silently dropped the
    /// sequence and the clipboard kept its old contents.
    ///
    /// Returns `true` when a copy was actually attempted (`Ok(..)`), so
    /// callers know they may clear the source selection.
    pub fn report_clipboard_result(&mut self, result: Result<ClipboardOutcome>) -> bool {
        match result {
            Ok(outcome) => {
                if outcome.delivered() {
                    self.set_copy_notice();
                } else {
                    tracing::warn!(
                        "clipboard: OSC 52 sequence emitted, but TERM={:?} TERM_PROGRAM={:?} \
                         is not known to support it — the clipboard was not updated",
                        std::env::var("TERM").unwrap_or_default(),
                        std::env::var("TERM_PROGRAM").unwrap_or_default()
                    );
                    self.set_copy_error_notice();
                }
                true
            }
            Err(e) => {
                tracing::warn!("copy_to_clipboard failed: {e}");
                self.set_copy_error_notice();
                false
            }
        }
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
        self.render_state.wide_table_regions.clear();
        self.render_state.wide_table_keyboard_target = None;
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

    /// Number of display columns one horizontal scroll step moves a wide
    /// table: `max(8, viewport_width / 3)`. `viewport_width` is the width the
    /// target table was actually windowed to (its render viewport), not the
    /// outer message area, so the step matches what is on screen.
    pub fn table_horiz_scroll_step(viewport_width: usize) -> i64 {
        std::cmp::max(8, viewport_width / 3) as i64
    }

    /// Move the horizontal window of the message owning `region` by `delta`
    /// display columns (positive = right), clamped to `[0, region.max_offset]`.
    /// Marks the message render-dirty so it re-renders at the new window.
    /// Returns whether the window actually moved.
    ///
    /// The stored window is first re-based into this table's clamped range:
    /// several wide tables in one message share a single window, so the stored
    /// value can exceed the natural maximum of the table being scrolled. Using
    /// the clamped value as the starting point keeps panning monotone instead
    /// of jumping left on the first step.
    pub fn scroll_wide_table(&mut self, region: &WideTableScrollRegion, delta: i64) -> bool {
        let max_offset = region.max_offset as i64;
        self.with_rendered_message_mut(region.message_index, |message| {
            let current = message.table_horiz_offset.min(region.max_offset);
            let new_offset = (current as i64 + delta).clamp(0, max_offset) as usize;
            if new_offset == current {
                return false;
            }
            message.table_horiz_offset = new_offset;
            message.mark_render_dirty();
            true
        })
        .unwrap_or(false)
    }

    /// Keyboard-directed horizontal scroll of the topmost visible wide table.
    /// `direction` is -1 (left) or +1 (right); the step comes from
    /// [`AppState::table_horiz_scroll_step`]. Returns whether the window moved.
    pub fn scroll_active_wide_table(&mut self, direction: i64) -> bool {
        let Some(region) = self.render_state.wide_table_keyboard_target else {
            return false;
        };
        let step = Self::table_horiz_scroll_step(region.viewport_width);
        self.scroll_wide_table(&region, direction.saturating_mul(step))
    }

    /// Subagent lane currently rendered as the transcript, if any.
    ///
    /// Mirrors the list selection in `render::transcript::render_chat`: a
    /// stack entry whose lane no longer exists falls back to the main chat,
    /// so callers never address a lane that is not on screen.
    fn active_rendered_agent_id(&self) -> Option<String> {
        self.chat_state
            .active_subagent_id()
            .filter(|agent_id| self.chat_state.subagent_lanes.contains_key(*agent_id))
            .map(ToOwned::to_owned)
    }

    /// Run `f` on the message at `message_index` in the transcript currently
    /// on screen (the active subagent lane, else the main chat). Returns
    /// `None` when that transcript has no such message.
    fn with_rendered_message_mut<R>(
        &mut self,
        message_index: usize,
        f: impl FnOnce(&mut Message) -> R,
    ) -> Option<R> {
        if let Some(agent_id) = self.active_rendered_agent_id() {
            self.chat_state
                .subagent_lanes
                .get_mut(&agent_id)?
                .messages
                .get_mut(message_index)
                .map(f)
        } else {
            self.chat_state.messages.get_mut(message_index).map(f)
        }
    }

    /// True while a transcript drag-selection is in progress: a selection
    /// exists and the scrollbar is not being dragged. In this state the mouse
    /// wheel and drags past the box edges scroll the transcript *and* extend
    /// the selection to the content now under the pointer, so a single drag
    /// can cover (and, on release, auto-copy) content that spans beyond the
    /// visible viewport instead of only what is currently on screen.
    pub fn transcript_drag_active(&self) -> bool {
        self.transcript_selection.is_some() && !self.active_transcript_scrollbar_dragging()
    }

    /// Re-map a screen position to transcript text and move the in-progress
    /// selection's cursor there.
    ///
    /// The position is first clamped into the Messages content area, so
    /// events from beyond the box's edges — a drag that ran past the bottom
    /// into the input box, or a pointer above the top edge — map to the
    /// first/last visible line instead of being dropped. Called after each
    /// scroll step while a drag-selection is active: the content slides
    /// under the stationary pointer, and the selection is re-extended to
    /// whatever now sits there.
    pub fn extend_transcript_selection_to(&mut self, column: u16, row: u16, area: Rect) {
        let (column, row) = clamp_to_transcript_content(column, row, area);
        let scroll_offset = self.active_transcript_scroll_offset();
        if let Some(sel) = self.transcript_selection.as_mut() {
            let (line_idx, col) = mouse_to_line_col(
                column,
                row,
                area,
                scroll_offset,
                self.render_state.transcript_cache.as_ref(),
            );
            sel.cursor_line = line_idx;
            sel.cursor_col = col;
        }
    }

    pub fn plan_panel_scroll_up(&mut self) {
        self.plan_panel.scroll_up();
    }

    pub fn plan_panel_scroll_down(&mut self) {
        self.plan_panel.scroll_down();
    }

    pub fn plan_panel_max_scroll_offset(&self) -> usize {
        self.plan_panel.max_scroll_offset()
    }

    pub fn set_plan_panel_scroll_offset(&mut self, line_offset: usize) {
        self.plan_panel.set_scroll_offset(line_offset);
    }

    pub fn plan_panel_scrollbar_dragging(&self) -> bool {
        self.plan_panel.scrollbar_dragging
    }

    pub fn set_plan_panel_scrollbar_dragging(&mut self, dragging: bool) {
        self.plan_panel.scrollbar_dragging = dragging;
    }

    pub fn clear_tool_file_baselines(&mut self) {
        self.diff_tracker.clear_tool_file_baselines();
    }

    /// High-level entry: tool transitioned to Running.
    pub fn on_tool_running(&mut self, call_id: &str, tool: &str, args_preview: &str) {
        self.diff_tracker
            .on_tool_running(call_id, tool, args_preview);
    }

    /// High-level entry: pre-execution file baseline captured
    /// synchronously by the tool-event sink on the executor thread
    /// (before the tool ran), delivered via
    /// `SessionTurnUpdate::ToolBaseline`.
    pub fn inject_tool_file_baseline(
        &mut self,
        call_id: &str,
        payload: xiaoo_shared::session_diff::ToolFileBaselinePayload,
    ) {
        self.diff_tracker
            .inject_session_file_baseline(call_id, &payload);
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
        let file_change_str = match &file_change {
            Some(delta) => format!(
                "Some(file_path={}, additions={}, deletions={})",
                delta.file_path, delta.additions, delta.deletions
            ),
            None => "None".to_string(),
        };
        let _ = crate::support::error_log::append_error_log(
            "app_state on_tool_completed",
            &format!(
                "call_id={call_id} tool={tool} args_preview={args_preview} file_change={file_change_str}"
            ),
        );
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
        // File-mention candidates are built against the workspace root; a cd
        // invalidates any cached list until the next input change refreshes it.
        self.file_mention.cached_token = None;
        self.file_mention.candidates.clear();
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

    pub fn file_mention_menu_visible(&self) -> bool {
        if self.is_subagent_view_active() || self.interaction_prompt.is_some() {
            return false;
        }
        if self.input_mode != InputMode::Editing || self.chat_state.is_loading {
            return false;
        }
        if self.slash_menu_visible() {
            return false;
        }
        let Some(token) = self.file_mention_current_token() else {
            return false;
        };
        if self
            .file_mention
            .dismissed_prefix
            .as_deref()
            .is_some_and(|dismissed| dismissed == &token.typed)
        {
            return false;
        }
        !self.file_mention_candidates().is_empty()
    }

    fn file_mention_current_token(&self) -> Option<FileMentionToken> {
        file_mention_token(
            self.chat_state.input.value(),
            self.chat_state.input.cursor(),
        )
    }

    pub fn file_mention_candidates(&self) -> &[FileMentionCandidate] {
        let Some(current) = self.file_mention_current_token() else {
            return &[];
        };
        if self
            .file_mention
            .cached_token
            .as_ref()
            .is_some_and(|cached| cached == &current)
        {
            return &self.file_mention.candidates;
        }
        &[]
    }

    pub fn file_mention_candidate_count(&self) -> usize {
        self.file_mention_candidates().len()
    }

    pub fn file_mention_selected_candidates(&self) -> Vec<FileMentionCandidate> {
        self.file_mention_candidates().to_vec()
    }

    pub fn refresh_file_mention_candidates(&mut self) {
        let Some(current) = self.file_mention_current_token() else {
            self.file_mention.cached_token = None;
            self.file_mention.candidates.clear();
            return;
        };
        if self
            .file_mention
            .cached_token
            .as_ref()
            .is_some_and(|cached| cached == &current)
        {
            return;
        }
        let candidates =
            file_mention_candidates(&self.workspace, &current.typed, FILE_MENTION_MAX_CANDIDATES);
        self.file_mention.selected = self
            .file_mention
            .selected
            .min(candidates.len().saturating_sub(1));
        self.file_mention.candidates = candidates;
        self.file_mention.cached_token = Some(current);
    }

    pub fn apply_file_mention_selection(&mut self) {
        let Some(chosen) = self
            .file_mention
            .candidates
            .get(self.file_mention.selected)
            .map(|c| c.path.clone())
        else {
            return;
        };
        crate::input::file_mention::apply_file_mention_pick(&mut self.chat_state.input, &chosen);
        self.chat_state.reset_input_history_navigation();
        self.file_mention.dismissed_prefix = Some(chosen);
        self.file_mention.cached_token = None;
        self.file_mention.candidates.clear();
        self.note_input_changed();
    }

    pub fn dismiss_current_file_mention_menu(&mut self) {
        self.file_mention.dismissed_prefix =
            self.file_mention_current_token().map(|token| token.typed);
        self.file_mention.cached_token = None;
        self.file_mention.candidates.clear();
    }

    pub fn note_input_changed(&mut self) {
        let value = self.chat_state.input.value();
        let cursor = self.chat_state.input.cursor();

        let prefix = slash_typed_prefix(value, cursor);
        let mention_token = file_mention_token(value, cursor);

        if self
            .slash
            .dismissed_prefix
            .as_deref()
            .is_some_and(|dismissed| prefix.as_deref() != Some(dismissed))
        {
            self.slash.dismissed_prefix = None;
        }

        if mention_token.as_ref().map(|token| token.typed.as_str())
            != self.file_mention.dismissed_prefix.as_deref()
        {
            self.file_mention.dismissed_prefix = None;
        }
        if self
            .file_mention
            .cached_token
            .as_ref()
            .is_some_and(|cached| Some(cached) != mention_token.as_ref())
        {
            self.file_mention.cached_token = None;
            self.file_mention.candidates.clear();
        }
        self.refresh_file_mention_candidates();

        let candidate_count = self.slash_candidate_count();
        if candidate_count > 0 {
            self.slash.selected = self.slash.selected.min(candidate_count - 1);
        }
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

/// Clamp a mouse position into the Messages content area: text starts one
/// column after the left border and ends before the scrollbar track, and
/// rows are bounded by the box itself. Positions from beyond the edges
/// (a drag that ran past the bottom into the input box, a pointer above
/// the top edge, a wheel event near a border) map to the first/last
/// visible line/char instead of being dropped.
fn clamp_to_transcript_content(column: u16, row: u16, area: Rect) -> (u16, u16) {
    let last_row = area.y.saturating_add(area.height).saturating_sub(1);
    let clamped_row = if last_row >= area.y {
        row.clamp(area.y, last_row)
    } else {
        area.y
    };
    let content_left = area.x.saturating_add(1);
    let content_right = area.x.saturating_add(area.width.saturating_sub(3));
    let clamped_column = if content_right >= content_left {
        column.clamp(content_left, content_right)
    } else {
        content_left
    };
    (clamped_column, clamped_row)
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
        Some("linux_dynsandbox") => "dynsandbox",
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
        Some("linux_dynsandbox") => "Dyn-Sandbox",
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
        "dynsandbox" => {
            object.insert(
                "isolation".to_string(),
                serde_json::json!({
                    "kind": "linux_dynsandbox"
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
#[path = "../../../../tests/unit/endside/state/app_state_test.rs"]
mod tests;
