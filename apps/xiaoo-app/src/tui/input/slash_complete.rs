//! Slash-command completion for the TUI chat input. Must stay in sync with dispatch in
//! `app.rs` (`/connect`, `/create-skill`, `/dir`, `/prompt-demo`).

use crate::input::Input;

pub struct SlashCommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
}

/// Canonical slash commands (ASCII only).
pub const SLASH_COMMANDS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "/connect",
        summary: "打开 provider / model 选择窗口并连接当前后端。",
    },
    SlashCommandSpec {
        name: "/create-skill",
        summary: "引导 agent 生成一个新的 skill。",
    },
    SlashCommandSpec {
        name: "/dir",
        summary: "切换当前工作目录。",
    },
    SlashCommandSpec {
        name: "/prompt-demo",
        summary: "打开内置的交互式 prompt 示例窗口。",
    },
];

/// Prefix of the current line being edited (from `/` through cursor), if the line starts a slash command.
pub fn slash_typed_prefix(value: &str, cursor: usize) -> Option<String> {
    let (line_start, line_end) = line_char_bounds(value, cursor);
    let line_len = line_end.saturating_sub(line_start);
    let line_before_cursor = cursor.saturating_sub(line_start).min(line_len);
    let line_chars: Vec<char> = value.chars().skip(line_start).take(line_len).collect();
    let before_cursor: String = line_chars.iter().take(line_before_cursor).collect();
    let lead_ws = before_cursor
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();
    let typed: String = before_cursor.chars().skip(lead_ws).collect();
    if typed.starts_with('/') {
        Some(typed)
    } else {
        None
    }
}

/// Commands whose canonical form starts with `typed` (ASCII case-insensitive).
pub fn candidates_for_prefix(typed: &str) -> Vec<&'static str> {
    SLASH_COMMANDS
        .iter()
        .map(|spec| spec.name)
        .filter(|cmd| {
            cmd.to_ascii_lowercase()
                .starts_with(typed.to_ascii_lowercase().as_str())
        })
        .collect()
}

pub fn summary_for_command(command: &str) -> Option<&'static str> {
    SLASH_COMMANDS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(command))
        .map(|spec| spec.summary)
}

/// Replace the slash token on the current line with `chosen` (full command string).
pub fn apply_slash_pick(input: &mut Input, chosen: &str) {
    let value = input.value();
    let cursor = input.cursor();
    let (line_start, line_end) = line_char_bounds(value, cursor);
    let line_len = line_end.saturating_sub(line_start);
    let line_before_cursor = cursor.saturating_sub(line_start).min(line_len);
    let line_chars: Vec<char> = value.chars().skip(line_start).take(line_len).collect();
    let before_cursor: String = line_chars.iter().take(line_before_cursor).collect();
    let lead_ws = before_cursor
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();
    let replace_start = line_start + lead_ws;
    let replace_end = cursor;
    let new_value: String = value
        .chars()
        .take(replace_start)
        .chain(chosen.chars())
        .chain(value.chars().skip(replace_end))
        .collect();
    let new_cursor = replace_start + chosen.chars().count();
    *input = Input::default()
        .with_value(new_value)
        .with_cursor(new_cursor);
}

/// Tab: longest-prefix expand (bash-style), or single match. Returns `true` if the buffer changed.
pub fn apply_slash_tab(input: &mut Input) -> bool {
    let value = input.value();
    let cursor = input.cursor();
    let Some(typed) = slash_typed_prefix(value, cursor) else {
        return false;
    };
    let candidates: Vec<&str> = candidates_for_prefix(&typed);
    if candidates.is_empty() {
        return false;
    }
    let new_token = if candidates.len() == 1 {
        candidates[0].to_string()
    } else {
        longest_common_prefix(&candidates)
    };
    if new_token == typed {
        return false;
    }
    apply_slash_pick(input, &new_token);
    true
}

fn line_char_bounds(s: &str, cursor: usize) -> (usize, usize) {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let cursor = cursor.min(n);
    let mut line_start = 0usize;
    for i in 0..cursor {
        if chars[i] == '\n' {
            line_start = i + 1;
        }
    }
    let mut line_end = n;
    for j in line_start..n {
        if chars[j] == '\n' {
            line_end = j;
            break;
        }
    }
    (line_start, line_end)
}

fn longest_common_prefix(strs: &[&str]) -> String {
    if strs.is_empty() {
        return String::new();
    }
    let first = strs[0].as_bytes();
    let mut len = first.len();
    for s in &strs[1..] {
        let b = s.as_bytes();
        len = len.min(
            first
                .iter()
                .zip(b.iter())
                .take_while(|(a, c)| a == c)
                .count(),
        );
    }
    strs[0][..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn con_to_connect() {
        let mut i: Input = "/con".into();
        assert!(apply_slash_tab(&mut i));
        assert_eq!(i.value(), "/connect");
    }

    #[test]
    fn c_stays_ambiguous() {
        let mut i: Input = "/c".into();
        assert!(!apply_slash_tab(&mut i));
        assert_eq!(i.value(), "/c");
    }

    #[test]
    fn create_s_to_skill() {
        let mut i: Input = "/create-s".into();
        assert!(apply_slash_tab(&mut i));
        assert_eq!(i.value(), "/create-skill");
    }

    #[test]
    fn leading_spaces() {
        let mut i = Input::default().with_value("  /con".to_string());
        assert!(apply_slash_tab(&mut i));
        assert_eq!(i.value(), "  /connect");
    }

    #[test]
    fn candidates_prefix() {
        assert_eq!(
            candidates_for_prefix("/"),
            vec!["/connect", "/create-skill", "/dir", "/prompt-demo"]
        );
        assert_eq!(
            candidates_for_prefix("/c"),
            vec!["/connect", "/create-skill"]
        );
        assert_eq!(candidates_for_prefix("/con"), vec!["/connect"]);
        assert_eq!(candidates_for_prefix("/d"), vec!["/dir"]);
        assert_eq!(candidates_for_prefix("/p"), vec!["/prompt-demo"]);
    }

    #[test]
    fn apply_pick() {
        let mut i: Input = "/co".into();
        apply_slash_pick(&mut i, "/connect");
        assert_eq!(i.value(), "/connect");
    }

    #[test]
    fn summaries_are_available_for_known_commands() {
        assert_eq!(
            summary_for_command("/connect"),
            Some("打开 provider / model 选择窗口并连接当前后端。")
        );
        assert_eq!(summary_for_command("/missing"), None);
    }
}
