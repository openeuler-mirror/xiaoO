use super::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Paragraph, Widget, Wrap};

#[test]
fn test_basic() {
    let (row, col) = calculate_visual_cursor_position("hello", 3, 10);
    assert_eq!((row, col), (0, 3));
}

#[test]
fn test_newline() {
    let (row, col) = calculate_visual_cursor_position("hello\nworld", 6, 10);
    assert_eq!((row, col), (1, 0));
}

#[test]
fn test_cursor_after_newline() {
    let (row, col) = calculate_visual_cursor_position("hello\n", 6, 10);
    assert_eq!((row, col), (1, 0));
}

#[test]
fn test_empty() {
    let (row, col) = calculate_visual_cursor_position("", 0, 10);
    assert_eq!((row, col), (0, 0));
}

#[test]
fn test_multiple_spaces_wrap() {
    // "aa   bb" at width 4: ratatui renders "aa  " / "bb  " (two trailing
    // spaces fill the first line as background). Cursor at the end (7) must
    // be on row 1 at the end of "bb" (col 2).
    assert_eq!(calculate_visual_cursor_position("aa   bb", 7, 4), (1, 2));
    // cursor before the first 'b' lands at the start of row 1.
    assert_eq!(calculate_visual_cursor_position("aa   bb", 5, 4), (1, 0));
}

#[test]
fn test_long_word_break() {
    // "aaaaaaaa" at width 4 wraps as "aaaa" / "aaaa"; cursor=4 is the start
    // of the second line, not the (off-screen) end of the first.
    assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 4, 4), (1, 0));
    assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 8, 4), (1, 4));
}

#[test]
fn test_cjk_wrap() {
    // Each CJK char is width 2; width 6 fits three per line.
    assert_eq!(calculate_visual_cursor_position("中文测试", 3, 6), (1, 0));
    assert_eq!(calculate_visual_cursor_position("中文测试", 4, 6), (1, 2));
}

/// Differential test: for every cursor offset, the (row, col) returned
/// must match the cell where ratatui's `Paragraph` + `Wrap { trim: false }`
/// actually renders that character (off-screen columns are skipped, since
/// the renderer clamps them separately).
fn ratatui_cell(value: &str, width: u16, row: usize, col: usize) -> String {
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height: 80,
    };
    let mut buf = Buffer::empty(area);
    Widget::render(
        Paragraph::new(value).wrap(Wrap { trim: false }),
        area,
        &mut buf,
    );
    buf[(col as u16, row as u16)].symbol().to_string()
}

fn assert_matches_ratatui(value: &str, width: usize) {
    let chars: Vec<char> = value.chars().collect();
    for p in 0..=chars.len() {
        let (row, col) = calculate_visual_cursor_position(value, p, width);
        assert!(
            col <= width,
            "value={value:?} width={width} cursor={p}: col {col} > width {width}"
        );
        if p < chars.len() {
            let ch = chars[p];
            if ch == '\n' {
                continue;
            }
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w == 0 || col == width {
                // zero-width chars and exact-fill boundary cursors have no
                // unique on-screen cell to compare against.
                continue;
            }
            let cell = ratatui_cell(value, width as u16, row, col);
            let cell_char = cell.chars().next();
            assert_eq!(
                cell_char,
                Some(ch),
                "value={value:?} width={width} cursor={p} (char {ch:?}): \
                     calc=({row},{col}) but ratatui cell is {cell:?}"
            );
        }
    }
}

#[test]
fn diff_basic_word() {
    assert_matches_ratatui("hello", 10);
}

#[test]
fn diff_long_single_word_wraps_mid_word() {
    assert_matches_ratatui("aaaaaaaa", 4);
}

#[test]
fn diff_word_wrap_with_spaces() {
    assert_matches_ratatui("aa bb cc dd", 4);
}

#[test]
fn diff_multiple_spaces_boundary() {
    assert_matches_ratatui("aa   bb", 4);
}

#[test]
fn diff_cursor_in_trailing_whitespace() {
    assert_matches_ratatui("aaaa ", 4);
}

#[test]
fn diff_newline_then_wrap() {
    assert_matches_ratatui("hello\naaaaaaa", 4);
}

#[test]
fn diff_cjk_wrap() {
    assert_matches_ratatui("中文测试一二三四五六", 6);
}

#[test]
fn diff_spaces_then_newline() {
    assert_matches_ratatui("aa  \nbb", 4);
}

#[test]
fn diff_long_paragraph() {
    assert_matches_ratatui(
        "The quick brown fox jumps over the lazy dog and keeps going",
        12,
    );
}

#[test]
fn diff_consecutive_newlines() {
    assert_matches_ratatui("a\n\nb", 4);
}
