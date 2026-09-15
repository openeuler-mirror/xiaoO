use super::*;

#[test]
fn find_actual_string_exact_match() {
    assert_eq!(
        find_actual_string("hello world\nfoo bar\n", "foo bar"),
        Some("foo bar".to_string())
    );
}

#[test]
fn find_actual_string_curly_quote_normalization() {
    let content = "x = 'value'";
    let search = "x = \u{2018}value\u{2019}";
    assert_eq!(
        find_actual_string(content, search),
        Some("x = 'value'".to_string())
    );
}

#[test]
fn find_actual_string_whitespace_tolerant_internal_spacing() {
    let content = "def f():\n    x  =  1\n    return x\n";
    let search = "x = 1";
    assert_eq!(
        find_actual_string(content, search),
        Some("    x  =  1".to_string())
    );
}

#[test]
fn find_actual_string_whitespace_tolerant_trailing_whitespace() {
    let content = "if cond:   \n    pass  \n";
    let search = "if cond:\n    pass";
    let got = find_actual_string(content, search).expect("should salvage");
    assert!(content.contains(&got));
    assert!(got.contains("pass"));
}

#[test]
fn find_actual_string_whitespace_tolerant_returns_findable_substring() {
    let content = "    foo  ()\n    bar()\n";
    let search = "foo ()\nbar()";
    let actual = find_actual_string(content, search).expect("should salvage");
    assert!(content.contains(&actual));
}

#[test]
fn find_actual_string_whitespace_tolerant_ambiguous_returns_none() {
    let content = "    foo  ()\nfunc:\n    foo   ()\n";
    let search = "foo ()";
    assert_eq!(find_actual_string(content, search), None);
}

#[test]
fn find_actual_string_whitespace_tolerant_blank_only_search_rejected() {
    let content = "line1\n\n\nline2\n";
    let search = "  \n  ";
    assert_eq!(find_actual_string(content, search), None);
}

#[test]
fn find_actual_string_no_match_returns_none() {
    assert_eq!(find_actual_string("hello\n", "xyzzy"), None);
}

#[test]
fn apply_edit_uses_whitespace_salvaged_string() {
    let content = "def f():\n    x  =  1\n";
    let search = "x = 1";
    assert!(!content.contains(search), "test must exercise salvage path");
    let actual = find_actual_string(content, search).expect("should salvage");
    let updated = apply_edit_to_file(content, &actual, "    x = 2", false).expect("replace");
    assert_eq!(updated, "def f():\n    x = 2\n");
}
