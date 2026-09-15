use super::wrap_text_to_lines;

#[test]
fn preserves_hard_newlines() {
    assert_eq!(
        wrap_text_to_lines("line one\nline two", 100),
        vec!["line one".to_string(), "line two".to_string()]
    );
}

#[test]
fn keeps_blank_line_for_double_newline() {
    assert_eq!(
        wrap_text_to_lines("a\n\nb", 100),
        vec!["a".to_string(), String::new(), "b".to_string()]
    );
}

#[test]
fn wraps_each_segment_by_width() {
    assert_eq!(
        wrap_text_to_lines("abc\nabcdef", 4),
        vec!["abc".to_string(), "abcd".to_string(), "ef".to_string()]
    );
}

#[test]
fn empty_input_returns_single_empty_line() {
    assert_eq!(wrap_text_to_lines("", 10), vec![String::new()]);
}
