use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use crate::input::{Input, InputRequest};

pub fn scroll_offset_from_drag(rel_y: usize, track_height: usize, max_scroll: usize) -> usize {
    if track_height <= 1 {
        return max_scroll;
    }
    let denominator = track_height.saturating_sub(1);
    max_scroll.saturating_mul(rel_y) / denominator
}

pub fn paste_into_input(input: &mut Input, text: &str) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for ch in normalized.chars() {
        input.handle(InputRequest::InsertChar(ch));
    }
}

pub(crate) fn cursor_row_col(value: &str, cursor: usize) -> (usize, usize) {
    let chars: Vec<char> = value.chars().collect();
    let length = chars.len();
    let cursor = cursor.min(length);
    let mut row = 0usize;
    let mut line_start = 0usize;
    for idx in 0..cursor {
        if chars[idx] == '\n' {
            row += 1;
            line_start = idx + 1;
        }
    }
    let col = cursor - line_start;
    (row, col)
}

pub(crate) fn line_prefix_width(line: &str, col_chars: usize) -> usize {
    line.chars()
        .take(col_chars)
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

pub(crate) fn truncate_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > max_width {
            if used < max_width {
                output.push('…');
            }
            return output;
        }
        output.push(ch);
        used += width;
    }
    output
}

pub(crate) fn render_tool_detail_text(text: &str) -> String {
    text.replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\n")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

pub(crate) fn rendered_line_count(lines: &[Line<'_>], width: u16) -> usize {
    lines
        .iter()
        .map(|line| {
            let visual_width: usize = line
                .spans
                .iter()
                .flat_map(|span| span.content.chars())
                .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
                .sum();
            if width == 0 {
                0
            } else {
                visual_width.max(1).div_ceil(width as usize)
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_line_count_matches_paragraph_wrap_for_indented_content() {
        let lines = vec![Line::from("               4 Indent")];

        assert_eq!(rendered_line_count(&lines, 10), 3);
    }

    #[test]
    fn scroll_offset_from_drag_reaches_bottom_at_last_row() {
        assert_eq!(scroll_offset_from_drag(0, 12, 40), 0);
        assert_eq!(scroll_offset_from_drag(11, 12, 40), 40);
    }

    #[test]
    fn render_tool_detail_text_decodes_escaped_newlines() {
        assert_eq!(
            render_tool_detail_text("line1\\nline2\\r\\nline3\\rline4"),
            "line1\nline2\nline3\nline4"
        );
    }
}
