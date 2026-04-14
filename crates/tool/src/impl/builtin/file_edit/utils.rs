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
    None
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
