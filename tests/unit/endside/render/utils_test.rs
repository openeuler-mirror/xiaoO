use super::*;
use crate::input::Input;

#[test]
fn paste_after_click_keeps_leading_char() {
    // Regression for the exact reported bug: mouse-click the input box
    // (sets cursor + anchor on an empty input), then paste a path —
    // the leading "/" was eaten because the second InsertChar saw the
    // stale anchor as a selection and replaced the first char.
    let mut input = Input::default();
    input.set_cursor(0);
    input.set_anchor(); // click artifact: anchor == cursor == 0
    paste_into_input(&mut input, "/root/code/atom/xiaoO");

    assert_eq!(input.value(), "/root/code/atom/xiaoO");
    assert_eq!(input.cursor(), 21);
    assert!(input.selected_text().is_none());
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
fn sanitize_terminal_text_downgrades_wide_table_scroll_arrows() {
    // The wide-table hint line (`◀…▶ cols a-b/n Alt+←/→`) must be measurable
    // in its rendered form: every glyph it uses has an ASCII expansion.
    assert_eq!(
        sanitize_terminal_text_for_mode("◀…▶ Alt+←/→", true),
        "<...> Alt+<-/->"
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
