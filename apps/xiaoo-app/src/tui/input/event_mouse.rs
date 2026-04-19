use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::interaction_prompt::PromptFocus;
use crate::provider_service::copy_to_clipboard;
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
        self.handle_transcript_mouse(mouse_event);
        Ok(())
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
                self.state.transcript_selection = None;
                self.state.chat_state.scroll_up();
            }
            MouseEventKind::ScrollDown => {
                self.state.transcript_selection = None;
                self.state.chat_state.scroll_down();
            }
            MouseEventKind::Down(MouseButton::Left) if in_scrollbar_zone => {
                self.state.chat_state.scrollbar_dragging = true;
            }
            // Right-click: copy whatever is currently selected (like opencode's right-click copy).
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(text) = self.state.transcript_selected_text() {
                    if let Err(e) = copy_to_clipboard(&text) {
                        tracing::warn!("copy_to_clipboard failed: {}", e);
                    } else {
                        self.state.set_copy_notice();
                    }
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

                // Check tool toggle first.
                if let Some(region) = self
                    .state
                    .render_state
                    .tool_toggle_regions
                    .iter()
                    .find(|region| mouse_in_rect(mouse_event.column, mouse_event.row, region.rect))
                    .copied()
                {
                    if let Some(message) =
                        self.state.chat_state.messages.get_mut(region.message_index)
                    {
                        if let Some(tool) = message.tool_state.as_mut() {
                            tool.expanded = !tool.expanded;
                        }
                    }
                    return;
                }

                // Start a new transcript selection.
                let (line_idx, col) = mouse_to_line_col(
                    mouse_event.column,
                    mouse_event.row,
                    area,
                    self.state.chat_state.scroll_offset,
                    &self.state.render_state.line_texts,
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
                if let Some(sel) = self.state.transcript_selection.as_mut() {
                    let (line_idx, col) = mouse_to_line_col(
                        mouse_event.column,
                        mouse_event.row,
                        area,
                        self.state.chat_state.scroll_offset,
                        &self.state.render_state.line_texts,
                    );
                    sel.cursor_line = line_idx;
                    sel.cursor_col = col;
                }
            }
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                if self.state.chat_state.scrollbar_dragging =>
            {
                let track_height = area.height as usize;
                let max_scroll = self.state.chat_state.max_scroll_offset();
                if track_height > 0 && max_scroll > 0 {
                    let rel_y = (mouse_event.row.saturating_sub(area.y) as usize)
                        .min(track_height.saturating_sub(1));
                    self.state
                        .chat_state
                        .set_scroll_offset(scroll_offset_from_drag(
                            rel_y,
                            track_height,
                            max_scroll,
                        ));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.state.chat_state.scrollbar_dragging = false;
                // Auto copy-on-select: mirrors opencode's onMouseUp handler.
                // Any non-empty selection is automatically copied when the mouse is released,
                // and the selection is cleared to confirm the action.
                if let Some(text) = self.state.transcript_selected_text() {
                    if let Err(e) = copy_to_clipboard(&text) {
                        tracing::warn!("copy_to_clipboard failed: {}", e);
                    } else {
                        self.state.set_copy_notice();
                    }
                    self.state.transcript_selection = None;
                } else if self
                    .state
                    .transcript_selection
                    .as_ref()
                    .is_some_and(|s| s.is_empty())
                {
                    self.state.transcript_selection = None;
                }
            }
            _ => {}
        }
    }
}

fn mouse_in_rect(column: u16, row: u16, rect: Rect) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

/// Convert a mouse (column, row) terminal position into a `(logical_line_index, char_col)`
/// pair within the flat logical-line array (`line_texts`).
///
/// `area` is the `messages_area` rect (scrollbar_area: outer x/width, inner y/height).
/// `scroll_offset` is the current vertical scroll in **visual rows** (matching
/// `rendered_line_count`).  `line_texts` holds one entry per *logical* line;
/// ratatui wraps long logical lines across multiple visual rows, so the mapping
/// must account for that.  CJK characters with display-width 2 are handled via
/// `UnicodeWidthChar`.
fn mouse_to_line_col(
    column: u16,
    row: u16,
    area: Rect,
    scroll_offset: usize,
    line_texts: &[String],
) -> (usize, usize) {
    if line_texts.is_empty() {
        return (0, 0);
    }

    // Absolute visual row from the top of the full content.
    let rel_row = row.saturating_sub(area.y) as usize;
    let visual_row = scroll_offset.saturating_add(rel_row);

    // Text content width: the `area` rect is the scrollbar_area which has the
    // full outer block width; subtract 2 for the left and right borders.
    // This matches the width passed to `rendered_line_count` / `Paragraph::wrap`.
    let content_width = (area.width as usize).saturating_sub(2).max(1);

    // Column within the text content (the left border is 1 terminal column wide).
    let col_in_content = column.saturating_sub(area.x.saturating_add(1)) as usize;

    // Walk logical lines, accumulating their visual-row counts, until we find
    // the logical line that contains `visual_row`.
    let mut visual_rows_so_far = 0usize;
    for (logical_idx, text) in line_texts.iter().enumerate() {
        let chars: Vec<char> = text.chars().collect();

        // Display width of this logical line (sum of per-character widths).
        let display_width: usize = chars
            .iter()
            .map(|ch| UnicodeWidthChar::width(*ch).unwrap_or(0))
            .sum();

        // A blank line still occupies one visual row.
        let line_visual_rows = (display_width.max(1) + content_width - 1) / content_width;

        if visual_rows_so_far + line_visual_rows > visual_row {
            // This logical line contains the target visual row.
            let row_within_line = visual_row - visual_rows_so_far;

            // Number of display columns to skip (all wrapped rows before this one).
            let skip_display = row_within_line * content_width;

            // Advance to the char that starts this visual row.
            let mut disp = 0usize;
            let mut char_idx = 0usize;
            while char_idx < chars.len() && disp < skip_display {
                disp += UnicodeWidthChar::width(chars[char_idx]).unwrap_or(0);
                char_idx += 1;
            }

            // Then advance col_in_content more display columns.
            let target_disp = disp + col_in_content;
            while char_idx < chars.len() && disp < target_disp {
                disp += UnicodeWidthChar::width(chars[char_idx]).unwrap_or(0);
                char_idx += 1;
            }

            return (logical_idx, char_idx.min(chars.len()));
        }
        visual_rows_so_far += line_visual_rows;
    }

    // Past all lines – clamp to the last character of the last line.
    let last_idx = line_texts.len() - 1;
    let last_col = line_texts[last_idx].chars().count();
    (last_idx, last_col)
}
