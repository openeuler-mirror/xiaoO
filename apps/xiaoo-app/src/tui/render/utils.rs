use std::sync::OnceLock;

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

fn use_ascii_terminal_symbols() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        cfg!(windows)
            || std::env::var_os("WT_SESSION").is_some()
            || std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::env::var_os("ConEmuPID").is_some()
    })
}

pub(crate) fn sanitize_terminal_text(text: &str) -> String {
    sanitize_terminal_text_for_mode(text, use_ascii_terminal_symbols())
}

fn sanitize_terminal_text_for_mode(text: &str, ascii_mode: bool) -> String {
    if !ascii_mode {
        return text.to_string();
    }

    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '▎' | '▌' | '│' => output.push('|'),
            '▾' => output.push('v'),
            '▸' => output.push('>'),
            '⟡' => output.push('*'),
            '⭕' => output.push('o'),
            '◔' => output.push_str("[-]"),
            '•' => output.push('*'),
            '●' => output.push('o'),
            '✅' => output.push_str("[x]"),
            '✓' => output.push('x'),
            '☐' | '□' => output.push_str("[ ]"),
            '→' => output.push_str("->"),
            '←' => output.push_str("<-"),
            '—' | '–' => output.push('-'),
            '…' => output.push_str("..."),
            '─' => output.push('-'),
            '⠋' | '⠼' | '⠇' => output.push('|'),
            '⠙' | '⠴' | '⠏' => output.push('/'),
            '⠹' | '⠦' => output.push('-'),
            '⠸' | '⠧' => output.push('\\'),
            _ => output.push(ch),
        }
    }
    output
}

pub(crate) fn truncate_display_width(text: &str, max_width: usize) -> String {
    truncate_display_width_for_mode(text, max_width, use_ascii_terminal_symbols())
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn truncate_display_width_for_mode(text: &str, max_width: usize, ascii_mode: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    let text = sanitize_terminal_text_for_mode(text, ascii_mode);
    let ellipsis = sanitize_terminal_text_for_mode("…", ascii_mode);
    let total_width = display_width(&text);
    if total_width <= max_width {
        return text;
    }

    let ellipsis_width = display_width(&ellipsis);
    if ellipsis_width >= max_width {
        let mut output = String::new();
        let mut used = 0usize;
        for ch in text.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + width > max_width {
                break;
            }
            output.push(ch);
            used += width;
        }
        return output;
    }

    let keep_width = max_width.saturating_sub(ellipsis_width);
    let mut output = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > keep_width {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push_str(&ellipsis);
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
    fn sanitize_terminal_text_replaces_problematic_unicode_symbols() {
        assert_eq!(
            sanitize_terminal_text_for_mode("▎ ✅ ◔ ⟡ │ • → …", true),
            "| [x] [-] * | * -> ..."
        );
    }

    #[test]
    fn truncate_display_width_uses_ascii_ellipsis_in_ascii_mode() {
        assert_eq!(truncate_display_width_for_mode("abcdef", 5, true), "ab...");
    }

    #[test]
    fn render_tool_detail_text_decodes_escaped_newlines() {
        assert_eq!(
            render_tool_detail_text("line1\\nline2\\r\\nline3\\rline4"),
            "line1\nline2\nline3\nline4"
        );
    }
}
