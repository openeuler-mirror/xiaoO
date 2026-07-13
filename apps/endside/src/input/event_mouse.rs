use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::app_state::{AppState, TranscriptRenderCache};
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
                self.state.active_transcript_scroll_up();
            }
            MouseEventKind::ScrollDown => {
                self.state.transcript_selection = None;
                self.state.active_transcript_scroll_down();
            }
            MouseEventKind::Down(MouseButton::Left) if in_scrollbar_zone => {
                self.state.set_active_transcript_scrollbar_dragging(true);
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
                let scroll_offset = self.state.active_transcript_scroll_offset();
                if let Some(sel) = self.state.transcript_selection.as_mut() {
                    let (line_idx, col) = mouse_to_line_col(
                        mouse_event.column,
                        mouse_event.row,
                        area,
                        scroll_offset,
                        self.state.render_state.transcript_cache.as_ref(),
                    );
                    sel.cursor_line = line_idx;
                    sel.cursor_col = col;
                }
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

fn active_message_mut(
    state: &mut AppState,
    message_index: usize,
) -> Option<&mut crate::chat::Message> {
    if let Some(agent_id) = state.chat_state.active_subagent_id().map(ToOwned::to_owned) {
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
fn mouse_to_line_col(
    column: u16,
    row: u16,
    area: Rect,
    scroll_offset: usize,
    cache: Option<&TranscriptRenderCache>,
) -> (usize, usize) {
    let Some(cache) = cache else {
        return (0, 0);
    };
    if cache.line_texts.is_empty() || cache.visual_lines.is_empty() {
        return (0, 0);
    }

    // Absolute visual row from the top of the full content.
    let rel_row = row.saturating_sub(area.y) as usize;
    let visual_row = scroll_offset.saturating_add(rel_row);

    // Column within the text content (the left border is 1 terminal column wide).
    let col_in_content = column.saturating_sub(area.x.saturating_add(1)) as usize;

    // Past all visual lines – clamp to the last character of the last logical line.
    if visual_row >= cache.visual_lines.len() {
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
        let visual_text: String = cache.visual_lines[v]
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
    let char_idx_within_visual =
        visual_line_char_at_display_col(&cache.visual_lines[visual_row], col_in_content);

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

#[cfg(test)]
mod tests {
    use super::mouse_to_line_col;
    use crate::app_state::CachedMessageRender;
    use crate::render::build_transcript_cache;
    use crate::selection::TranscriptSelection;
    use crate::theme::Theme;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    /// Regression test for the "click lands on the wrong line/char" bug.
    ///
    /// Before the fix, `mouse_to_line_col` recomputed wrapping via
    /// `div_ceil(display_width, content_width)`, which disagreed with
    /// `wrap_line_to_visual_lines` (textwrap word-aware) for lines containing
    /// long paths/URLs. A long filesystem path at width 40 is a canonical
    /// case: textwrap splits the path into 3 visual rows, while the old
    /// character-based predictor only accounted for 2 — so a click on the 3rd
    /// visual row ("ame.json)") was mapped to the wrong logical line.
    ///
    /// After the fix, `mouse_to_line_col` reads the actual `visual_lines` /
    /// `logical_line_visual_starts` from the `TranscriptRenderCache` built by
    /// `render_chat`, so the mouse→text mapping always agrees with what's
    /// drawn on screen.
    #[test]
    fn mouse_to_line_col_maps_click_on_wrapped_path_tail_correctly() {
        // Logical lines of a System message rendered by `render_standard_message_lines`:
        //   header  → "  ▎ System  HH:MM:SS"  (1 visual row)
        //   content → "  Session snapshot saved: name (/tmp/.../snapshot-name.json)"
        //              textwrap at width 40 → 3 visual rows (path is one long
        //              unbreakable token that textwrap splits at the boundary)
        //   empty   → ""                     (1 visual row)
        // Synthetic long path — no dependency on any developer's home dir or
        // filesystem layout. The path (in parens) is 45 chars > width 40, so
        // textwrap splits it at the width boundary.
        let content = "Session snapshot saved: name (/tmp/xiaoo-test/sessions/snapshot-name.json)";
        let render = CachedMessageRender {
            revision: 0,
            width: 40,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![
                Line::from("  ▎ System  12:00:00"),
                Line::from(format!("  {content}")),
                Line::raw(""),
            ],
        };
        let cache = build_transcript_cache(&[render]);

        // The content logical line is index 1, starting at visual row 1
        // (header is row 0). It wraps to 3 visual rows (rows 1, 2, 3),
        // and the empty spacer is row 4.
        assert_eq!(cache.logical_line_visual_starts[1], 1);
        assert_eq!(cache.visual_lines.len(), 5);

        let logical_text: String = cache.line_texts[1].chars().collect();
        let area = Rect::new(0, 0, 42, 10);

        // Dynamically determine the tail text from the actual wrapped visual
        // row at visual_row = 3 (the 3rd visual row of the content line — the
        // path suffix that textwrap split off).  This keeps the test robust
        // against path-length changes.
        let tail_text: String = cache.visual_lines[3]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let tail_chars: Vec<char> = tail_text.chars().collect();
        assert!(
            !tail_chars.is_empty(),
            "path tail visual row must not be empty"
        );

        // The content area starts at column 1 (after the left border), so:
        //   terminal column 1 → content col 0 → tail_chars[0]
        //   terminal column 2 → content col 1 → tail_chars[1]
        //   terminal column 3 → content col 2 → tail_chars[2]  (if it exists)
        let cases: &[(u16, usize)] = &[(1, 0), (2, 1), (3, 2)];
        for &(terminal_col, tail_idx) in cases {
            if tail_idx >= tail_chars.len() {
                continue;
            }
            let expected_ch = tail_chars[tail_idx];
            let (line_idx, char_col) = mouse_to_line_col(terminal_col, 3, area, 0, Some(&cache));
            assert_eq!(
                line_idx, 1,
                "click on path tail (col {terminal_col}) must map to content logical line"
            );
            let ch_at_col: Option<char> = logical_text.chars().nth(char_col);
            assert_eq!(
                ch_at_col,
                Some(expected_ch),
                "click at terminal col {terminal_col} → char_col {char_col} should be '{expected_ch}'"
            );
        }

        // Also verify: clicking on the first char of the path tail maps to the
        // exact position of `tail_text` within the full logical-line text.
        let tail_pos = logical_text
            .find(&tail_text)
            .unwrap_or_else(|| panic!("logical text must contain path tail {tail_text:?}"));
        let (line_idx, char_col_for_first) = mouse_to_line_col(1, 3, area, 0, Some(&cache));
        assert_eq!(line_idx, 1);
        assert_eq!(
            char_col_for_first, tail_pos,
            "click on first char of path tail must map to the exact char index in the logical text"
        );
    }

    /// Clicking past the last visual line clamps to the last logical line's end.
    #[test]
    fn mouse_to_line_col_past_end_clamps_to_last_line() {
        let render = CachedMessageRender {
            revision: 0,
            width: 80,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![Line::from("hello"), Line::raw("")],
        };
        let cache = build_transcript_cache(&[render]);

        let area = Rect::new(0, 0, 80, 10);
        // Row 100 is way past the last visual line.
        let (line_idx, char_col) = mouse_to_line_col(5, 100, area, 0, Some(&cache));
        assert_eq!(line_idx, 1, "past-end click clamps to last logical line");
        assert_eq!(char_col, 0, "empty spacer line has 0 chars");
    }

    /// When no cache is available (e.g., before the first render), the function
    /// returns (0, 0) instead of panicking.
    #[test]
    fn mouse_to_line_col_returns_zero_when_cache_is_none() {
        let area = Rect::new(0, 0, 80, 10);
        let (line_idx, char_col) = mouse_to_line_col(5, 5, area, 0, None);
        assert_eq!(line_idx, 0);
        assert_eq!(char_col, 0);
    }

    /// CJK content triggers `is_special_width_line` → `wrap_line_by_character`
    /// (per-character wrapping, a distinct code path from the textwrap
    /// word-aware path). The header line stays on the textwrap path, so this
    /// test exercises both paths within the same cache and verifies the mouse
    /// mapping is correct for the CJK tail row.
    #[test]
    fn mouse_to_line_col_maps_click_on_cjk_wrapped_line() {
        // Header is kept short so it fits on one row at width 10 — isolates
        // the test to the CJK content line's wrapping behavior.
        // Content: 2 leading ASCII spaces + 12 CJK chars (display width 2 each)
        // = total display width 2 + 24 = 26. At render width 10:
        //   v0 = "  你好世界" (2 + 4*2 = 10)  → 6 chars at pos 0
        //   v1 = "你好世界你" (5*2 = 10)       → 5 chars at pos 6
        //   v2 = "好世界"    (3*2 = 6)         → 3 chars at pos 11
        let render = CachedMessageRender {
            revision: 0,
            width: 10,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![
                Line::from("Hdr"),
                Line::from("  你好世界你好世界你好世界"),
                Line::raw(""),
            ],
        };
        let cache = build_transcript_cache(&[render]);

        // Layout: header(1) + content(3 visual rows) + spacer(1) = 5.
        assert_eq!(cache.logical_line_visual_starts[1], 1);
        assert_eq!(cache.visual_lines.len(), 5);

        let logical_text: String = cache.line_texts[1].chars().collect();
        let area = Rect::new(0, 0, 12, 10);

        // Click on v2 (visual row 3), first char. v2 starts at logical
        // char 11 (after 2 spaces + 9 CJK chars from v0+v1).
        let (line_idx, char_col) = mouse_to_line_col(1, 3, area, 0, Some(&cache));
        assert_eq!(line_idx, 1, "click on v2 must map to content logical line");
        assert_eq!(
            char_col, 11,
            "click on first char of v2 must map to char 11 in logical text"
        );
        assert_eq!(
            logical_text.chars().nth(char_col),
            Some('好'),
            "char at mapped position must be '好' (start of v2)"
        );
    }

    /// Symmetric to `mouse_to_line_col_maps_click_on_wrapped_path_tail_correctly`
    /// but clicks on the FIRST visual row of a multi-wrap content line. This
    /// covers the path where the loop `for v in line_start_visual..visual_row`
    /// is empty (visual_row == line_start_visual) and char_offset stays 0.
    #[test]
    fn mouse_to_line_col_maps_click_on_first_visual_row_of_wrapped_line() {
        // Reuse the long-path content from the tail-click test, but click on
        // visual row 1 (v0 of content) instead of row 3 (v2).
        let content = "Session snapshot saved: name (/tmp/xiaoo-test/sessions/snapshot-name.json)";
        let render = CachedMessageRender {
            revision: 0,
            width: 40,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![
                Line::from("  ▎ System  12:00:00"),
                Line::from(format!("  {content}")),
                Line::raw(""),
            ],
        };
        let cache = build_transcript_cache(&[render]);

        let logical_text: String = cache.line_texts[1].chars().collect();
        let area = Rect::new(0, 0, 42, 10);

        // Click on visual row 1 (content v0), col 1 (terminal col 1 →
        // content col 0). The line's first char is ' ' (leading space).
        let (line_idx, char_col) = mouse_to_line_col(1, 1, area, 0, Some(&cache));
        assert_eq!(line_idx, 1, "click on v0 must map to content logical line");
        assert_eq!(
            char_col, 0,
            "click on first char of v0 must map to char 0 in logical text"
        );
        assert_eq!(logical_text.chars().nth(char_col), Some(' '));

        // Click one column to the right (terminal col 2 → content col 1).
        // Content col 1 is the second leading space.
        let (line_idx2, char_col2) = mouse_to_line_col(2, 1, area, 0, Some(&cache));
        assert_eq!(line_idx2, 1);
        assert_eq!(char_col2, 1);
        assert_eq!(logical_text.chars().nth(char_col2), Some(' '));
    }

    /// Repeated-substring regression: when a visual line's text appears
    /// multiple times in the logical line (e.g. "hello hello hello hello"
    /// wrapped to one word per row), `find_substring_from` must advance past
    /// the stripped inter-word spaces and land on the correct occurrence, not
    /// always the first.
    #[test]
    fn mouse_to_line_col_handles_repeated_substrings() {
        // 4× "hello" separated by single spaces. At width 5 (each word fits
        // exactly), textwrap emits 4 visual rows of "hello" (inter-word
        // spaces stripped). Logical char positions of each "hello":
        //   v0 @ 0, v1 @ 6, v2 @ 12, v3 @ 18.
        let render = CachedMessageRender {
            revision: 0,
            width: 5,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![Line::from("hello hello hello hello")],
        };
        let cache = build_transcript_cache(&[render]);

        // 4 visual rows, all on logical line 0.
        assert_eq!(cache.visual_lines.len(), 4);
        assert_eq!(cache.logical_line_visual_starts, vec![0]);

        let logical_text: String = cache.line_texts[0].chars().collect();
        let area = Rect::new(0, 0, 7, 10);

        // Click first char of each visual row; verify char_col advances by
        // 6 each time (5 for "hello" + 1 for the stripped space).
        for (visual_row, expected_offset) in [(0, 0), (1, 6), (2, 12), (3, 18)] {
            let (line_idx, char_col) = mouse_to_line_col(1, visual_row, area, 0, Some(&cache));
            assert_eq!(line_idx, 0, "all visual rows must map to logical line 0");
            assert_eq!(
                char_col, expected_offset,
                "visual row {visual_row} must map to char {expected_offset}"
            );
            assert_eq!(
                logical_text.chars().nth(char_col),
                Some('h'),
                "char at mapped position must be 'h' (start of word {visual_row})"
            );
        }
    }

    /// Header lines (`line_is_header = true`) must still produce a valid
    /// (line_idx, char_col) without panicking; the caller relies on
    /// `transcript_selected_text` later filtering them out during copy.
    #[test]
    fn mouse_to_line_col_on_header_line_does_not_panic() {
        let render = CachedMessageRender {
            revision: 0,
            width: 40,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![Line::from("  ▎ You  12:00:00"), Line::raw("body")],
        };
        let cache = build_transcript_cache(&[render]);

        // Sanity: header is the first logical line.
        assert_eq!(cache.line_is_header, vec![true, false]);

        let area = Rect::new(0, 0, 42, 10);
        // Click in the middle of the header row.
        let (line_idx, char_col) = mouse_to_line_col(5, 0, area, 0, Some(&cache));
        assert_eq!(line_idx, 0, "click on header maps to logical line 0");
        // No panic; char_col is a valid index ≤ header text length.
        let header_text: String = cache.line_texts[0].chars().collect();
        assert!(
            char_col <= header_text.chars().count(),
            "char_col {char_col} must be within header text bounds"
        );
    }

    /// Drag selection across wrapped visual rows of the same logical line.
    /// Builds a `TranscriptSelection` from two `mouse_to_line_col` calls
    /// (anchor on v0, cursor on v2) and verifies the normalised bounds are
    /// within the same logical line and ordered correctly.
    #[test]
    fn mouse_to_line_col_drag_across_wrapped_visual_rows() {
        let content = "Session snapshot saved: name (/tmp/xiaoo-test/sessions/snapshot-name.json)";
        let render = CachedMessageRender {
            revision: 0,
            width: 40,
            theme: Theme::for_test(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            lines: vec![
                Line::from("  ▎ System  12:00:00"),
                Line::from(format!("  {content}")),
                Line::raw(""),
            ],
        };
        let cache = build_transcript_cache(&[render]);

        let area = Rect::new(0, 0, 42, 10);

        // Anchor: click near the start of content v0 (row 1, col 1).
        let (anchor_line, anchor_col) = mouse_to_line_col(1, 1, area, 0, Some(&cache));
        // Cursor: drag to content v2 (row 3, col 5).
        let (cursor_line, cursor_col) = mouse_to_line_col(5, 3, area, 0, Some(&cache));

        let mut sel = TranscriptSelection::new(anchor_line, anchor_col);
        sel.cursor_line = cursor_line;
        sel.cursor_col = cursor_col;
        let (start_line, start_col, end_line, end_col) = sel.normalised();

        assert_eq!(start_line, 1, "drag stays within content logical line");
        assert_eq!(end_line, 1);
        assert!(
            start_col < end_col,
            "normalised start_col {start_col} must be < end_col {end_col}"
        );
        // Anchor on v0 should map to a smaller char offset than cursor on v2.
        assert_eq!(start_col, anchor_col);
        assert_eq!(end_col, cursor_col);
    }
}
