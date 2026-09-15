use super::input_char_index_at;
use super::mouse_to_line_col;
use super::wide_table_at;
use crate::app_state::{CachedMessageRender, WideTableScrollRegion};
use crate::render::transcript::build_transcript_cache;
use crate::render::wrap_line_to_visual_lines;
use crate::selection::TranscriptSelection;
use ratatui::layout::Rect;
use ratatui::text::Line;

/// Wide-table hit region occupying rows `y..y+height` of a full-width area.
fn wide_region(
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    viewport_width: usize,
) -> WideTableScrollRegion {
    WideTableScrollRegion {
        message_index: 0,
        rect: Rect::new(x, y, width, height),
        viewport_width,
        max_offset: 60,
    }
}

#[test]
fn wide_table_at_finds_the_table_under_the_pointer() {
    let regions = vec![wide_region(0, 5, 40, 6, 40), wide_region(0, 20, 40, 4, 40)];

    // Inside the first table (its first and last row).
    assert_eq!(wide_table_at(&regions, 0, 5).map(|r| r.rect.y), Some(5));
    assert_eq!(wide_table_at(&regions, 39, 10).map(|r| r.rect.y), Some(5));
    // Inside the second one.
    assert_eq!(wide_table_at(&regions, 12, 20).map(|r| r.rect.y), Some(20));
    assert_eq!(wide_table_at(&regions, 12, 23).map(|r| r.rect.y), Some(20));
    // Between the two, past the right edge, past the bottom edge: no hit.
    assert!(wide_table_at(&regions, 12, 15).is_none());
    assert!(wide_table_at(&regions, 40, 6).is_none());
    assert!(wide_table_at(&regions, 12, 24).is_none());
}

#[test]
fn wide_table_at_prefers_the_topmost_overlapping_region() {
    // Regions are collected top-to-bottom; an overlap must resolve to the
    // one drawn on top (the first pushed).
    let regions = vec![wide_region(0, 5, 40, 10, 40), wide_region(0, 8, 40, 10, 40)];
    assert_eq!(wide_table_at(&regions, 3, 9).map(|r| r.rect.y), Some(5));
}

#[test]
fn wide_table_at_degenerate_zero_width_region_never_hits() {
    let regions = vec![wide_region(0, 5, 0, 0, 0)];
    assert!(wide_table_at(&regions, 0, 5).is_none());
    assert!(wide_table_at(&[], 0, 5).is_none());
}

fn cached(lines: Vec<Line<'static>>, width: u16) -> CachedMessageRender {
    let wrapped_lines: Vec<Vec<Line<'static>>> = lines
        .iter()
        .map(|line| wrap_line_to_visual_lines(line, width))
        .collect();
    CachedMessageRender {
        width,
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
        wrapped_lines: Some(wrapped_lines),
        lines,
        frozen_prefix_line_count: None,
    }
}

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
    let render = cached(
        vec![
            Line::from("  ▎ System  12:00:00"),
            Line::from(format!("  {content}")),
            Line::raw(""),
        ],
        40,
    );

    let cache = build_transcript_cache(None, vec![Some(render)]);

    // The content logical line is index 1, starting at visual row 1
    // (header is row 0). It wraps to 3 visual rows (rows 1, 2, 3),
    // and the empty spacer is row 4.
    assert_eq!(cache.logical_line_visual_starts[1], 1);
    assert_eq!(cache.total_lines, 5);

    let logical_text: String = cache.line_texts[1].chars().collect();
    let area = Rect::new(0, 0, 42, 10);

    // Dynamically determine the tail text from the actual wrapped visual
    // row at visual_row = 3 (the 3rd visual row of the content line — the
    // path suffix that textwrap split off).  This keeps the test robust
    // against path-length changes.
    let tail_text: String = cache
        .visual_line(3)
        .expect("visual row 3 must exist")
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
    let render = cached(vec![Line::from("hello"), Line::raw("")], 80);

    let cache = build_transcript_cache(None, vec![Some(render)]);

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
    let render = cached(
        vec![
            Line::from("Hdr"),
            Line::from("  你好世界你好世界你好世界"),
            Line::raw(""),
        ],
        10,
    );

    let cache = build_transcript_cache(None, vec![Some(render)]);

    // Layout: header(1) + content(3 visual rows) + spacer(1) = 5.
    assert_eq!(cache.logical_line_visual_starts[1], 1);
    assert_eq!(cache.total_lines, 5);

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
    let render = cached(
        vec![
            Line::from("  ▎ System  12:00:00"),
            Line::from(format!("  {content}")),
            Line::raw(""),
        ],
        40,
    );

    let cache = build_transcript_cache(None, vec![Some(render)]);

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
    let render = cached(vec![Line::from("hello hello hello hello")], 5);

    let cache = build_transcript_cache(None, vec![Some(render)]);

    // 4 visual rows, all on logical line 0.
    assert_eq!(cache.total_lines, 4);
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
    let render = cached(vec![Line::from("  ▎ You  12:00:00"), Line::raw("body")], 40);

    let cache = build_transcript_cache(None, vec![Some(render)]);

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

/// Drag selection across wrapped visual rows of the same logical line.    /// Builds a `TranscriptSelection` from two `mouse_to_line_col` calls
/// (anchor on v0, cursor on v2) and verifies the normalised bounds are
/// within the same logical line and ordered correctly.
#[test]
fn mouse_to_line_col_drag_across_wrapped_visual_rows() {
    let content = "Session snapshot saved: name (/tmp/xiaoo-test/sessions/snapshot-name.json)";
    let render = cached(
        vec![
            Line::from("  ▎ System  12:00:00"),
            Line::from(format!("  {content}")),
            Line::raw(""),
        ],
        40,
    );

    let cache = build_transcript_cache(None, vec![Some(render)]);

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

#[test]
fn input_char_index_maps_single_line() {
    // "hello" with max_width 80: col 0..5 map to char 0..5; beyond the
    // end clamps to the value length.
    assert_eq!(input_char_index_at("hello", 80, 0, 0), 0);
    assert_eq!(input_char_index_at("hello", 80, 0, 2), 2);
    assert_eq!(input_char_index_at("hello", 80, 0, 4), 4);
    assert_eq!(input_char_index_at("hello", 80, 0, 5), 5);
    assert_eq!(input_char_index_at("hello", 80, 0, 99), 5);
}

#[test]
fn input_char_index_handles_wrap_and_newline() {
    // max_width 4: "abcdef" wraps as "abcd" / "ef" (a,b,c,d = row 0;
    // e,f = row 1).
    assert_eq!(input_char_index_at("abcdef", 4, 0, 0), 0);
    assert_eq!(input_char_index_at("abcdef", 4, 0, 3), 3);
    assert_eq!(input_char_index_at("abcdef", 4, 1, 0), 4);
    assert_eq!(input_char_index_at("abcdef", 4, 1, 1), 5);
    // Hard newline starts a new row: 'c' at (1,0), 'd' at (1,1).
    assert_eq!(input_char_index_at("ab\ncd", 80, 1, 0), 3);
    assert_eq!(input_char_index_at("ab\ncd", 80, 1, 1), 4);
}

#[test]
fn input_char_index_handles_wide_chars() {
    // CJK chars are 2 display columns wide; max_width 4 fits two of them.
    assert_eq!(input_char_index_at("你好x", 4, 0, 0), 0);
    assert_eq!(input_char_index_at("你好x", 4, 0, 1), 0);
    assert_eq!(input_char_index_at("你好x", 4, 0, 2), 1);
    // "你好" fills row 0 (4 cols); "x" wraps to row 1.
    assert_eq!(input_char_index_at("你好x", 4, 1, 0), 2);
}
