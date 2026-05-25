use similar::{ChangeTag, TextDiff};

use super::output::{Hunk, StructuredPatch};

pub const LEFT_SINGLE_CURLY_QUOTE: char = '\u{2018}';
pub const RIGHT_SINGLE_CURLY_QUOTE: char = '\u{2019}';
pub const LEFT_DOUBLE_CURLY_QUOTE: char = '\u{201C}';
pub const RIGHT_DOUBLE_CURLY_QUOTE: char = '\u{201D}';

pub fn normalize_quotes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            LEFT_SINGLE_CURLY_QUOTE | RIGHT_SINGLE_CURLY_QUOTE => result.push('\''),
            LEFT_DOUBLE_CURLY_QUOTE | RIGHT_DOUBLE_CURLY_QUOTE => result.push('"'),
            c => result.push(c),
        }
    }
    result
}

pub fn find_actual_string(content: &str, search_string: &str) -> Option<String> {
    if content.contains(search_string) {
        return Some(search_string.to_string());
    }
    let normalized = normalize_quotes(search_string);
    if content.contains(&normalized) {
        return Some(normalized);
    }
    find_whitespace_tolerant_match(content, &normalized)
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_whitespace_tolerant_match(content: &str, search: &str) -> Option<String> {
    if search.is_empty() {
        return None;
    }
    let search_norm: Vec<String> =
        search.split('\n').map(collapse_whitespace).collect();
    let m = search_norm.len();
    if search_norm.iter().all(|l| l.is_empty()) {
        return None;
    }

    let bytes = content.as_bytes();
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut line_start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            lines.push((line_start, i));
            line_start = i + 1;
        }
    }
    lines.push((line_start, bytes.len()));

    if lines.len() < m {
        return None;
    }

    let lines_norm: Vec<String> = lines
        .iter()
        .map(|&(s, e)| collapse_whitespace(&content[s..e]))
        .collect();

    let mut found: Option<(usize, usize)> = None;
    for start in 0..=lines.len() - m {
        if (0..m).all(|j| lines_norm[start + j] == search_norm[j]) {
            if found.is_some() {
                return None;
            }
            found = Some((lines[start].0, lines[start + m - 1].1));
        }
    }

    found.map(|(s, e)| content[s..e].to_string())
}

pub fn preserve_quote_style(actual_old_string: &str, new_string: &str) -> String {
    let has_curly_single = actual_old_string.contains(LEFT_SINGLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_SINGLE_CURLY_QUOTE);
    let has_curly_double = actual_old_string.contains(LEFT_DOUBLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_DOUBLE_CURLY_QUOTE);

    if !has_curly_single && !has_curly_double {
        return new_string.to_string();
    }

    let mut result = String::with_capacity(new_string.len());
    let mut chars = new_string.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                if has_curly_single {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        result.push(RIGHT_SINGLE_CURLY_QUOTE);
                    } else {
                        result.push(LEFT_SINGLE_CURLY_QUOTE);
                    }
                } else {
                    result.push(c);
                }
            }
            '"' => {
                if has_curly_double {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        result.push(RIGHT_DOUBLE_CURLY_QUOTE);
                    } else {
                        result.push(LEFT_DOUBLE_CURLY_QUOTE);
                    }
                } else {
                    result.push(c);
                }
            }
            c => result.push(c),
        }
    }

    result
}

pub fn apply_edit_to_file(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Option<String> {
    if old_string.is_empty() && new_string.is_empty() {
        return Some(content.to_string());
    }

    if replace_all {
        if content.contains(old_string) {
            Some(content.replace(old_string, new_string))
        } else {
            None
        }
    } else if let Some(pos) = content.find(old_string) {
        let mut result = content.to_string();
        result.replace_range(pos..pos + old_string.len(), new_string);
        Some(result)
    } else {
        None
    }
}

#[allow(dead_code)]
pub fn strip_trailing_whitespace(s: &str) -> String {
    s.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn get_patch_for_edit(old_string: &str, new_string: &str) -> (StructuredPatch, String) {
    let diff = TextDiff::from_lines(old_string, new_string);

    let mut hunks = Vec::new();
    let mut updated_lines = Vec::new();
    let mut has_changes = false;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete | ChangeTag::Insert => {
                has_changes = true;
                let sign = if change.tag() == ChangeTag::Delete {
                    "-"
                } else {
                    "+"
                };
                updated_lines.push(format!("{}{}", sign, change));
            }
            ChangeTag::Equal => {
                updated_lines.push(format!(" {}", change));
            }
        }
    }

    let old_lines_count = old_string.lines().count() as u32;
    let new_lines_count = new_string.lines().count() as u32;

    if has_changes {
        hunks.push(Hunk {
            old_start: 1,
            old_lines: old_lines_count,
            new_start: 1,
            new_lines: new_lines_count,
            lines: updated_lines,
        });
    }

    let updated_file = new_string.to_string();
    (hunks, updated_file)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(find_actual_string(content, search), Some("x = 'value'".to_string()));
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
        let updated =
            apply_edit_to_file(content, &actual, "    x = 2", false).expect("replace");
        assert_eq!(updated, "def f():\n    x = 2\n");
    }
}
