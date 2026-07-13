use std::sync::OnceLock;

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
            '●' | '○' => output.push('o'),
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
        .replace("\\t", "\t")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// Find the first occurrence of `needle` within `haystack` starting from the
/// `start`-th char. Returns the char index where the match begins. Uses
/// `str::find` (which respects UTF-8 char boundaries for valid UTF-8 needles)
/// so no `Vec<char>` allocation is needed.
///
/// Used to reverse-map a wrapped visual line's text back to its position in
/// the original logical line: textwrap strips inter-word whitespace at wrap
/// boundaries, so each visual line's text is a contiguous substring of the
/// original but not necessarily at a contiguous offset. Advancing via this
/// helper skips the stripped whitespace and lands on the correct char offset.
pub(crate) fn find_substring_from(haystack: &str, needle: &str, start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start);
    }
    let start_byte = haystack
        .char_indices()
        .nth(start)
        .map_or(haystack.len(), |(b, _)| b);
    let byte_pos = haystack[start_byte..].find(needle)?;
    Some(haystack[..start_byte + byte_pos].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn find_substring_from_skips_stripped_whitespace() {
        // Mirrors textwrap's behavior: "AAA BBB CCC" wrapped at width 4
        // produces ["AAA", "BBB", "CCC"] (inter-word spaces stripped). Each
        // visual line's text is a contiguous substring of the original, but
        // the offsets jump past the stripped spaces.
        let original = "AAA BBB CCC";
        // First visual line "AAA" at char 0.
        assert_eq!(find_substring_from(original, "AAA", 0), Some(0));
        // Second visual line "BBB": searching from char 3 (past "AAA" + the
        // space textwrap stripped) finds "BBB" at char 4, not 3.
        assert_eq!(find_substring_from(original, "BBB", 3), Some(4));
        // Third visual line "CCC": searching from char 7 (past "BBB" + the
        // stripped space) finds "CCC" at char 8.
        assert_eq!(find_substring_from(original, "CCC", 7), Some(8));
    }

    #[test]
    fn find_substring_from_handles_empty_needle_and_missing_match() {
        assert_eq!(find_substring_from("abc", "", 5), Some(5));
        assert_eq!(find_substring_from("abc", "xyz", 0), None);
        // UTF-8 char boundary safety: searching from char index 2 in "你好世界"
        // (which is the 3rd char '世') must land on a char boundary.
        assert_eq!(find_substring_from("你好世界", "世界", 0), Some(2));
    }
}
