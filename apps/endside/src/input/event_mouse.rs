use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::app_state::{AppState, InputMode, TranscriptRenderCache, WideTableScrollRegion};
use crate::interaction_prompt::PromptFocus;
use crate::provider_service::copy_to_clipboard;
use crate::render::find_substring_from;
use crate::render::scroll_offset_from_drag;
use crate::selection::TranscriptSelection;

impl App {
    pub(crate) fn handle_mouse_event(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if self.state.api_key_dialog.is_some() {
            return self.handle_api_key_dialog_mouse(mouse_event);
        }

        if self.state.provider_dialog.is_some() {
            return Ok(());
        }

        if self.state.interaction_prompt.is_some() {
            self.handle_interaction_prompt_mouse(mouse_event)?;
            return Ok(());
        }

        if self.state.file_mention_menu_visible() {
            if self.handle_header_mouse(mouse_event) {
                return Ok(());
            }
            self.handle_file_mention_popup_mouse(mouse_event)?;
            return Ok(());
        }

        if self.state.slash_menu_visible() {
            if self.handle_header_mouse(mouse_event) {
                return Ok(());
            }
            self.handle_slash_popup_mouse(mouse_event)?;
            return Ok(());
        }

        if self.handle_header_mouse(mouse_event) {
            return Ok(());
        }

        self.handle_slash_popup_mouse(mouse_event)?;
        self.handle_file_mention_popup_mouse(mouse_event)?;
        if self.handle_plan_panel_mouse(mouse_event)? {
            return Ok(());
        }

        self.handle_transcript_mouse(mouse_event);
        self.handle_input_mouse(mouse_event);
        Ok(())
    }

    /// Mouse drag-select on the input box. Terminals that intercept
    /// Ctrl+Shift+C / Ctrl+Insert (mate-terminal, alacritty, …) make
    /// keyboard copy unavailable, so the input box gets the same
    /// drag-select-then-auto-copy behaviour as the transcript.
    fn handle_input_mouse(&mut self, mouse_event: MouseEvent) {
        if self.state.input_mode != InputMode::Editing {
            self.input_drag_active = false;
            return;
        }
        if self.state.is_subagent_view_active() {
            self.input_drag_active = false;
            return;
        }
        // While a transcript drag-selection is in progress, the pointer may
        // pass over the input box (dragging past the Messages bottom edge
        // auto-scrolls the transcript and extends the selection). The caret
        // must not follow it, and the input box must not steal those drag
        // events. Any button press inside the input box clears the transcript
        // selection first (see the Down fall-through arm in
        // handle_transcript_mouse), so this guard only swallows the drags.
        if self.state.transcript_drag_active() {
            return;
        }
        let Some(inner) = self.state.render_state.input_area else {
            self.input_drag_active = false;
            return;
        };
        let in_area = mouse_event.column >= inner.x
            && mouse_event.column < inner.x + inner.width
            && mouse_event.row >= inner.y
            && mouse_event.row < inner.y + inner.height;

        // Map a mouse position inside the input content area to a character
        // index, accounting for the renderer's vertical scroll (scroll_y =
        // visual row of the cursor - (height - 1)).
        let char_index_at = |row: u16, col: u16| {
            let value = self.state.chat_state.input.value();
            let cursor = self.state.chat_state.input.cursor();
            let max_width = inner.width.max(1) as usize;
            let (visual_row, _) =
                crate::render::overlay::calculate_visual_cursor_position(value, cursor, max_width);
            let scroll_y = visual_row.saturating_sub(inner.height.max(1) as usize - 1);
            let content_row = row.saturating_sub(inner.y) as usize + scroll_y;
            let content_col = col.saturating_sub(inner.x) as usize;
            input_char_index_at(value, max_width, content_row, content_col)
        };

        match mouse_event.kind {
            MouseEventKind::Down(MouseButton::Left) if in_area => {
                self.input_drag_active = true;
                let index = char_index_at(mouse_event.row, mouse_event.column);
                // First click dismisses an existing selection and repositions
                // the caret (standard text-input behaviour — unlike the
                // transcript's dismiss-guard, the caret must follow the
                // click so the next keystroke lands where the user pointed).
                if self.state.chat_state.input.selected_range().is_some() {
                    self.state.chat_state.input.set_cursor(index);
                    self.state.chat_state.input.clear_selection();
                    return;
                }
                self.state.chat_state.input.set_cursor(index);
                self.state.chat_state.input.set_anchor();
            }
            // A left-button press anywhere else (tool toggle, scrollbar,
            // transcript area) ends any input-box drag: the subsequent Up
            // must not auto-copy a lingering input selection (e.g. one made
            // with Ctrl+A).
            MouseEventKind::Down(MouseButton::Left) => {
                self.input_drag_active = false;
            }
            MouseEventKind::Drag(MouseButton::Left) if in_area => {
                let index = char_index_at(mouse_event.row, mouse_event.column);
                self.state.chat_state.input.set_cursor(index);
            }
            // Auto-copy only when the drag actually started inside the input
            // box; releasing just outside still copies (anchor and cursor
            // were both set while inside).
            MouseEventKind::Up(MouseButton::Left) => {
                if self.input_drag_active {
                    self.input_drag_active = false;
                    if self.state.chat_state.input.selected_range().is_some() {
                        self.copy_active_selection();
                    }
                }
            }
            _ => {}
        }
    }

    /// Mouse handling for the sidebar Plan (current task list) panel.
    ///
    /// Returns `Ok(true)` when the event was consumed by the panel (wheel
    /// scroll inside the panel, scrollbar thumb press/drag/release, click on
    /// panel content), so it must not fall through to the transcript handler.
    fn handle_plan_panel_mouse(&mut self, mouse_event: MouseEvent) -> Result<bool> {
        // Continue/release an in-progress thumb drag regardless of the
        // cursor position, mirroring the transcript scrollbar drag.
        match mouse_event.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                if self.state.plan_panel_scrollbar_dragging() =>
            {
                if let Some(area) = self.state.render_state.plan_panel_area {
                    let track_height = area.height as usize;
                    let max_scroll = self.state.plan_panel_max_scroll_offset();
                    if track_height > 0 && max_scroll > 0 {
                        let rel_y = (mouse_event.row.saturating_sub(area.y) as usize)
                            .min(track_height.saturating_sub(1));
                        self.state
                            .set_plan_panel_scroll_offset(scroll_offset_from_drag(
                                rel_y,
                                track_height,
                                max_scroll,
                            ));
                    }
                }
                return Ok(true);
            }
            MouseEventKind::Up(MouseButton::Left) if self.state.plan_panel_scrollbar_dragging() => {
                self.state.set_plan_panel_scrollbar_dragging(false);
                return Ok(true);
            }
            _ => {}
        }

        let Some(area) = self.state.render_state.plan_panel_area else {
            return Ok(false);
        };
        if !mouse_in_rect(mouse_event.column, mouse_event.row, area) {
            return Ok(false);
        }

        // The scrollbar track occupies the rightmost 2 columns of the panel
        // (the thumb itself is drawn over the right border).
        let in_scrollbar_zone = mouse_event.column >= area.x + area.width.saturating_sub(2)
            && mouse_event.column < area.x + area.width;

        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                self.state.plan_panel_scroll_up();
                Ok(true)
            }
            MouseEventKind::ScrollDown => {
                self.state.plan_panel_scroll_down();
                Ok(true)
            }
            MouseEventKind::Down(MouseButton::Left) if in_scrollbar_zone => {
                self.state.set_plan_panel_scrollbar_dragging(true);
                // Jump the thumb to the clicked track position immediately.
                let track_height = area.height as usize;
                let max_scroll = self.state.plan_panel_max_scroll_offset();
                if track_height > 0 && max_scroll > 0 {
                    let rel_y = (mouse_event.row.saturating_sub(area.y) as usize)
                        .min(track_height.saturating_sub(1));
                    self.state
                        .set_plan_panel_scroll_offset(scroll_offset_from_drag(
                            rel_y,
                            track_height,
                            max_scroll,
                        ));
                }
                Ok(true)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Click on plan content: clear any transcript selection (the
                // click landed outside the transcript content zone) and
                // swallow the event.
                self.state.transcript_selection = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_api_key_dialog_mouse(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if mouse_event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(());
        }

        let Some(toggle_area) = self.state.render_state.api_key_toggle_area else {
            return Ok(());
        };

        if mouse_in_rect(mouse_event.column, mouse_event.row, toggle_area) {
            self.state.toggle_api_key_visibility();
        }

        Ok(())
    }

    fn handle_header_mouse(&mut self, mouse_event: MouseEvent) -> bool {
        if mouse_event.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }

        let Some(theme_toggle_area) = self.state.render_state.theme_toggle_area else {
            return false;
        };

        if !mouse_in_rect(mouse_event.column, mouse_event.row, theme_toggle_area) {
            return false;
        }

        self.state.toggle_theme();
        true
    }

    fn handle_interaction_prompt_mouse(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if self.state.interaction_prompt.is_none()
            || mouse_event.kind != MouseEventKind::Down(MouseButton::Left)
        {
            return Ok(());
        }

        if let Some(list_rect) = self.state.render_state.interaction_prompt_list_area {
            if mouse_in_rect(mouse_event.column, mouse_event.row, list_rect) {
                if let Some(prompt) = self.state.interaction_prompt.as_mut() {
                    prompt.focus = PromptFocus::List;
                    let row = (mouse_event.row.saturating_sub(list_rect.y)) as usize;
                    let visible_max = prompt.list_visible_max();
                    let index = prompt.list_scroll + row.min(visible_max.saturating_sub(1));
                    if index < prompt.request.choices.len() {
                        prompt.selected = index;
                    }
                }
                return Ok(());
            }
        }

        if let Some(supplement_rect) = self.state.render_state.interaction_prompt_supplement_area {
            if mouse_in_rect(mouse_event.column, mouse_event.row, supplement_rect) {
                if let Some(prompt) = self.state.interaction_prompt.as_mut() {
                    prompt.focus = PromptFocus::Supplement;
                }
            }
        }
        Ok(())
    }

    fn handle_slash_popup_mouse(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if mouse_event.kind != MouseEventKind::Down(MouseButton::Left)
            || !self.state.slash_menu_visible()
        {
            return Ok(());
        }
        if let Some(inner) = self.state.render_state.slash_popup_inner {
            if mouse_in_rect(mouse_event.column, mouse_event.row, inner) {
                let row = (mouse_event.row - inner.y) as usize;
                let value = self.state.chat_state.input.value();
                let cursor = self.state.chat_state.input.cursor();
                if let Some(prefix) = crate::slash_complete::slash_typed_prefix(value, cursor) {
                    let candidates = crate::slash_complete::candidates_for_prefix(
                        &prefix,
                        &self.state.external_commands,
                    );
                    if row < candidates.len() {
                        self.state.slash.selected = row;
                        self.state.apply_slash_selection();
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_file_mention_popup_mouse(&mut self, mouse_event: MouseEvent) -> Result<()> {
        if mouse_event.kind != MouseEventKind::Down(MouseButton::Left)
            || !self.state.file_mention_menu_visible()
        {
            return Ok(());
        }
        if let Some(inner) = self.state.render_state.file_mention_popup_inner {
            if mouse_in_rect(mouse_event.column, mouse_event.row, inner) {
                self.state.refresh_file_mention_candidates();
                let visible_row = (mouse_event.row - inner.y) as usize;
                let index = self.state.render_state.file_mention_view_start + visible_row;
                if index < self.state.file_mention_candidates().len() {
                    self.state.file_mention.selected = index;
                    self.state.apply_file_mention_selection();
                }
            }
        }
        Ok(())
    }

    fn handle_transcript_mouse(&mut self, mouse_event: MouseEvent) {
        let Some(area) = self.state.render_state.messages_area else {
            return;
        };
        let in_scrollbar_zone = mouse_event.column >= area.x + area.width.saturating_sub(2)
            && mouse_event.column < area.x + area.width
            && mouse_event.row >= area.y
            && mouse_event.row < area.y + area.height;
        let in_content_zone = !in_scrollbar_zone
            && mouse_event.column >= area.x
            && mouse_event.column < area.x + area.width.saturating_sub(2)
            && mouse_event.row >= area.y
            && mouse_event.row < area.y + area.height;

        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                // While a drag-selection is in progress, the wheel slides the
                // content under the stationary pointer and the selection
                // follows it: holding the button and rolling the wheel lets a
                // single drag cover (and auto-copy on release) content beyond
                // the visible viewport, not just what is on screen.
                if self.state.transcript_drag_active() {
                    self.state.active_transcript_scroll_up();
                    self.state
                        .extend_transcript_selection_to(mouse_event.column, mouse_event.row, area);
                } else {
                    self.state.transcript_selection = None;
                    self.state.active_transcript_scroll_up();
                }
            }
            MouseEventKind::ScrollDown => {
                // See ScrollUp: scroll-and-extend while a drag is active.
                if self.state.transcript_drag_active() {
                    self.state.active_transcript_scroll_down();
                    self.state
                        .extend_transcript_selection_to(mouse_event.column, mouse_event.row, area);
                } else {
                    self.state.transcript_selection = None;
                    self.state.active_transcript_scroll_down();
                }
            }
            // Shift+wheel commonly arrives as ScrollLeft/ScrollRight; when the
            // pointer hovers a windowed (wide) markdown table, pan it
            // horizontally instead of scrolling the transcript.
            MouseEventKind::ScrollLeft => {
                self.pan_hovered_wide_table(mouse_event.column, mouse_event.row, false);
            }
            MouseEventKind::ScrollRight => {
                self.pan_hovered_wide_table(mouse_event.column, mouse_event.row, true);
            }
            MouseEventKind::Down(MouseButton::Left) if in_scrollbar_zone => {
                self.state.set_active_transcript_scrollbar_dragging(true);
            }
            // Right-click: copy whatever is currently selected (like opencode's right-click copy).
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(text) = self.state.transcript_selected_text() {
                    self.state.report_clipboard_result(copy_to_clipboard(&text));
                    self.state.transcript_selection = None;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if in_content_zone => {
                // Selection protection: if a non-empty selection already exists, the first
                // click dismisses it without triggering tool toggles (mirrors opencode's
                // dismiss-guard on dialog / message click handlers).
                if self
                    .state
                    .transcript_selection
                    .as_ref()
                    .is_some_and(|s| !s.is_empty())
                {
                    self.state.transcript_selection = None;
                    return;
                }

                if let Some(region) = self
                    .state
                    .render_state
                    .subagent_open_regions
                    .iter()
                    .find(|region| mouse_in_rect(mouse_event.column, mouse_event.row, region.rect))
                    .cloned()
                {
                    self.state.enter_subagent_view(&region.agent_id);
                    return;
                }

                // Check tool toggle first.
                if let Some(region) = self
                    .state
                    .render_state
                    .tool_toggle_regions
                    .iter()
                    .find(|region| mouse_in_rect(mouse_event.column, mouse_event.row, region.rect))
                    .copied()
                {
                    if let Some(message) = active_message_mut(&mut self.state, region.message_index)
                    {
                        if let Some(tool) = message.tool_state.as_mut() {
                            tool.expanded = !tool.expanded;
                            message.mark_render_dirty();
                        }
                    }
                    return;
                }

                // Start a new transcript selection.
                let (line_idx, col) = mouse_to_line_col(
                    mouse_event.column,
                    mouse_event.row,
                    area,
                    self.state.active_transcript_scroll_offset(),
                    self.state.render_state.transcript_cache.as_ref(),
                );
                self.state.transcript_selection = Some(TranscriptSelection::new(line_idx, col));
                // Clear input selection when starting transcript selection.
                self.state.chat_state.input.clear_selection();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Clicked outside content (e.g. border); clear selection.
                self.state.transcript_selection = None;
            }
            MouseEventKind::Drag(MouseButton::Left) if in_content_zone => {
                self.state
                    .extend_transcript_selection_to(mouse_event.column, mouse_event.row, area);
            }
            // Drag-selection continued outside the content zone: above the top
            // edge or below the bottom edge of the Messages box (e.g. into
            // the input area). Auto-scroll one line in that direction and
            // extend the selection to the first/last visible line — the
            // classic drag-to-edge auto-scroll — so a single drag can cover
            // content spanning several screens. A position inside the box but
            // over the scrollbar columns simply extends the selection with
            // the column clamped into the text area.
            MouseEventKind::Drag(MouseButton::Left) if self.state.transcript_drag_active() => {
                if mouse_event.row >= area.y.saturating_add(area.height) {
                    self.state.active_transcript_scroll_down();
                } else if mouse_event.row < area.y {
                    self.state.active_transcript_scroll_up();
                }
                self.state
                    .extend_transcript_selection_to(mouse_event.column, mouse_event.row, area);
            }
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                if self.state.active_transcript_scrollbar_dragging() =>
            {
                let track_height = area.height as usize;
                let max_scroll = self.state.active_transcript_max_scroll_offset();
                if track_height > 0 && max_scroll > 0 {
                    let rel_y = (mouse_event.row.saturating_sub(area.y) as usize)
                        .min(track_height.saturating_sub(1));
                    self.state
                        .set_active_transcript_scroll_offset(scroll_offset_from_drag(
                            rel_y,
                            track_height,
                            max_scroll,
                        ));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.state.set_active_transcript_scrollbar_dragging(false);
                // Auto copy-on-select: mirrors opencode's onMouseUp handler.
                // Any non-empty selection is automatically copied when the mouse is released.
                if let Some(text) = self.state.transcript_selected_text() {
                    self.state.report_clipboard_result(copy_to_clipboard(&text));
                }
                // Always clear: a non-empty selection that yields no extractable text
                // (e.g. a drag confined to header lines) must not linger, otherwise the
                // dismiss-guard at the next Down swallows the first toggle click.
                self.state.transcript_selection = None;
            }
            _ => {}
        }
    }

    /// Pan the wide markdown table under the pointer, if any, by one step in
    /// the wheel's direction. `right` is true for a rightward pan.
    ///
    /// The step is derived from the hovered table's own render viewport
    /// (`WideTableScrollRegion::viewport_width`) rather than the outer message
    /// area, so tables rendered into a narrower window (e.g. indented tool
    /// output) still move by a third of what the user actually sees.
    fn pan_hovered_wide_table(&mut self, column: u16, row: u16, right: bool) {
        let Some(region) = wide_table_at(&self.state.render_state.wide_table_regions, column, row)
        else {
            return;
        };
        let step = AppState::table_horiz_scroll_step(region.viewport_width);
        self.state
            .scroll_wide_table(&region, if right { step } else { -step });
    }
}

/// Topmost visible wide table containing `(column, row)`, if any. Regions are
/// collected in visual order (top to bottom), so the first hit is the one the
/// user sees under the pointer.
fn wide_table_at(
    regions: &[WideTableScrollRegion],
    column: u16,
    row: u16,
) -> Option<WideTableScrollRegion> {
    regions
        .iter()
        .find(|region| mouse_in_rect(column, row, region.rect))
        .copied()
}

/// Message at `message_index` in the transcript currently on screen (the
/// active subagent lane, else the main chat). Mirrors the list selection in
/// `render::transcript::render_chat`: a stack entry whose lane no longer
/// exists falls back to the main chat, so a hit region is always resolved
/// against the same list it was built from.
fn active_message_mut(
    state: &mut AppState,
    message_index: usize,
) -> Option<&mut crate::chat::Message> {
    if let Some(agent_id) = state
        .chat_state
        .active_subagent_id()
        .filter(|agent_id| state.chat_state.subagent_lanes.contains_key(*agent_id))
        .map(ToOwned::to_owned)
    {
        return state
            .chat_state
            .subagent_lanes
            .get_mut(&agent_id)
            .and_then(|lane| lane.messages.get_mut(message_index));
    }
    state.chat_state.messages.get_mut(message_index)
}

fn mouse_in_rect(column: u16, row: u16, rect: Rect) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

/// Convert a mouse (column, row) terminal position into a `(logical_line_index, char_col)`
/// pair within the flat logical-line array.
///
/// `area` is the `messages_area` rect (scrollbar_area: outer x/width, inner y/height).
/// `scroll_offset` is the current vertical scroll in **visual rows** (matching
/// the `visual_lines` produced by `wrap_line_to_visual_lines`).
/// `cache` is the `TranscriptRenderCache` built during the last `render_chat`
/// frame — it carries the actual word-wrapped `visual_lines` and the
/// `logical_line_visual_starts` index, so the mouse→text mapping always agrees
/// with what's drawn on screen (previously this function recomputed wrapping
/// via `div_ceil(display_width, content_width)`, which diverged from textwrap's
/// word-aware wrapping for long paths/URLs and caused clicks to land on the
/// wrong character or the wrong line).
pub(crate) fn mouse_to_line_col(
    column: u16,
    row: u16,
    area: Rect,
    scroll_offset: usize,
    cache: Option<&TranscriptRenderCache>,
) -> (usize, usize) {
    let Some(cache) = cache else {
        return (0, 0);
    };
    if cache.line_texts.is_empty() || cache.total_lines == 0 {
        return (0, 0);
    }

    // Absolute visual row from the top of the full content.
    let rel_row = row.saturating_sub(area.y) as usize;
    let visual_row = scroll_offset.saturating_add(rel_row);

    // Column within the text content (the left border is 1 terminal column wide).
    let col_in_content = column.saturating_sub(area.x.saturating_add(1)) as usize;

    // Past all visual lines – clamp to the last character of the last logical line.
    if visual_row >= cache.total_lines {
        let last_idx = cache.line_texts.len() - 1;
        let last_col = cache.line_texts[last_idx].chars().count();
        return (last_idx, last_col);
    }

    // Binary-search the logical line whose visual-start row <= visual_row.
    // `logical_line_visual_starts[i]` = absolute visual row where logical line
    // `i` begins. `partition_point` returns the count of starts <= visual_row,
    // so subtracting 1 gives the logical line that owns `visual_row`.
    let logical_idx = cache
        .logical_line_visual_starts
        .partition_point(|&start| start <= visual_row)
        .saturating_sub(1)
        .min(cache.line_texts.len().saturating_sub(1));

    let line_start_visual = cache
        .logical_line_visual_starts
        .get(logical_idx)
        .copied()
        .unwrap_or(0);

    let logical_text = &cache.line_texts[logical_idx];

    // Walk the visual rows from the logical line's start through the clicked
    // row, matching each visual row's text against `logical_text` to
    // accumulate the correct char offset. This accounts for spaces that
    // textwrap trims at word-break boundaries — the sum of visual-row char
    // counts would otherwise under-count and drift the click mapping (e.g. a
    // long path that textwrap splits into 3 rows would have 1–2 trimmed
    // spaces, and the click on the 3rd row would land on the wrong character).
    // For rows before the clicked one, advance past the matched text; for the
    // clicked row itself, only align to its start position.
    let mut char_offset = 0usize;
    for v in line_start_visual..=visual_row {
        let Some(visual_line) = cache.visual_line(v) else {
            continue;
        };
        let visual_text: String = visual_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let Some(pos) = find_substring_from(logical_text, &visual_text, char_offset) else {
            continue;
        };
        char_offset = if v == visual_row {
            pos
        } else {
            pos + visual_text.chars().count()
        };
    }

    // Walk the clicked visual row's spans to find the char at `col_in_content`
    // display columns.
    let clicked_visual_line = cache
        .visual_line(visual_row)
        .expect("visual_row is in range (checked above)");
    let char_idx_within_visual =
        visual_line_char_at_display_col(clicked_visual_line, col_in_content);

    (logical_idx, char_offset + char_idx_within_visual)
}

/// Char index within a visual line at the given display column. Clamps to the
/// line's last char if `target_disp` is past the line's display width.
fn visual_line_char_at_display_col(line: &Line<'_>, target_disp: usize) -> usize {
    let mut disp = 0usize;
    let mut char_idx = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            if disp >= target_disp {
                return char_idx;
            }
            disp += UnicodeWidthChar::width(ch).unwrap_or(0);
            char_idx += 1;
        }
    }
    char_idx
}

/// Map a click inside the input content area to a character index of
/// `value`. Visual lines break at `\n` and at every `max_width` display
/// columns (a simplified approximation of the wrap used by the renderer —
/// word-boundary wrapping is not modelled; for typical input lengths the
/// difference is immaterial). `target_row`/`target_col` are relative to the
/// input content area (row 0, col 0 = first cell).
fn input_char_index_at(
    value: &str,
    max_width: usize,
    target_row: usize,
    target_col: usize,
) -> usize {
    let max_width = max_width.max(1);
    let mut row = 0usize;
    let mut col = 0usize;
    let mut char_idx = 0usize;
    for ch in value.chars() {
        if ch == '\n' {
            if row == target_row {
                return char_idx;
            }
            row += 1;
            col = 0;
            char_idx += 1;
            continue;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if col + w > max_width && col > 0 {
            row += 1;
            col = 0;
        }
        if row > target_row {
            return char_idx;
        }
        // The character occupies display columns [col, col + w); a click
        // inside that span (or at its left edge) maps to this character.
        if row == target_row && col + w > target_col {
            return char_idx;
        }
        col += w;
        char_idx += 1;
    }
    char_idx
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/input/event_mouse_test.rs"]
mod tests;
