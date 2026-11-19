use super::*;

/// (content, search, expected) — `find_actual_string` returns the verbatim
/// substring from the content, or a normalized salvage of it (curly quotes
/// → straight, whitespace-tolerant spacing); `None` when nothing matches,
/// the salvage would be ambiguous, or the search is blank-only.
#[test]
fn find_actual_string_matches_exact_and_normalized_forms() {
    let cases = [
        // exact match
        ("hello world\nfoo bar\n", "foo bar", Some("foo bar")),
        // curly quotes normalize to straight quotes
        (
            "x = 'value'",
            "x = \u{2018}value\u{2019}",
            Some("x = 'value'"),
        ),
        // whitespace-tolerant: internal spacing, indentation kept
        (
            "def f():\n    x  =  1\n    return x\n",
            "x = 1",
            Some("    x  =  1"),
        ),
        // ambiguous whitespace-tolerant match → None
        ("    foo  ()\nfunc:\n    foo   ()\n", "foo ()", None),
        // blank-only search is rejected
        ("line1\n\n\nline2\n", "  \n  ", None),
        // no match at all → None
        ("hello\n", "xyzzy", None),
    ];
    for (content, search, expected) in cases {
        assert_eq!(
            find_actual_string(content, search),
            expected.map(str::to_string),
            "search: {search:?}"
        );
    }
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
fn apply_edit_uses_whitespace_salvaged_string() {
    let content = "def f():\n    x  =  1\n";
    let search = "x = 1";
    assert!(!content.contains(search), "test must exercise salvage path");
    let actual = find_actual_string(content, search).expect("should salvage");
    let updated = apply_edit_to_file(content, &actual, "    x = 2", false).expect("replace");
    assert_eq!(updated, "def f():\n    x = 2\n");
}
