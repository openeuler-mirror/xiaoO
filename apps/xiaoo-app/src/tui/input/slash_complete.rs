//! Slash-command completion for the TUI chat input. Must stay in sync with dispatch in
//! `app.rs` (`/connect`, `/dir`, `/prompt-demo`).

use crate::input::Input;
use crate::services::command_loader::ExternalCommand;

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
    // NOTE: /create-skill is not yet implemented; hidden from TUI until ready.
    // SlashCommandSpec {
    //     name: "/create-skill",
    //     summary: "引导 agent 生成一个新的 skill。",
    // },
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
/// Includes both built-in commands and external commands from `~/.xiaoo/commands/`.
pub fn candidates_for_prefix(typed: &str, external: &[ExternalCommand]) -> Vec<String> {
    let typed_lower = typed.to_ascii_lowercase();
    let mut result: Vec<String> = SLASH_COMMANDS
        .iter()
        .map(|spec| spec.name)
        .filter(|cmd| cmd.to_ascii_lowercase().starts_with(&typed_lower))
        .map(|s| s.to_string())
        .collect();
    for cmd in external {
        let slash_name = format!("/{}", cmd.name);
        if slash_name.to_ascii_lowercase().starts_with(&typed_lower) {
            result.push(slash_name);
        }
    }
    result
}

/// Look up a summary/description for a command name, checking built-in then external.
pub fn summary_for_command(command: &str, external: &[ExternalCommand]) -> Option<String> {
    if let Some(spec) = SLASH_COMMANDS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(command))
    {
        return Some(spec.summary.to_string());
    }
    let name = command.strip_prefix('/').unwrap_or(command);
    external
        .iter()
        .find(|cmd| cmd.name.eq_ignore_ascii_case(name))
        .map(|cmd| cmd.description.clone())
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
pub fn apply_slash_tab(input: &mut Input, external: &[ExternalCommand]) -> bool {
    let value = input.value();
    let cursor = input.cursor();
    let Some(typed) = slash_typed_prefix(value, cursor) else {
        return false;
    };
    let candidates = candidates_for_prefix(&typed, external);
    if candidates.is_empty() {
        return false;
    }
    let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
    let new_token = if refs.len() == 1 {
        refs[0].to_string()
    } else {
        longest_common_prefix(&refs)
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

    const NO_EXT: &[ExternalCommand] = &[];

    fn sample_external() -> Vec<ExternalCommand> {
        vec![ExternalCommand {
            name: "agent-start".to_string(),
            description: "Start an agent project".to_string(),
            body: "invoke the agent-start skill".to_string(),
        }]
    }

    #[test]
    fn con_to_connect() {
        let mut i: Input = "/con".into();
        assert!(apply_slash_tab(&mut i, NO_EXT));
        assert_eq!(i.value(), "/connect");
    }

    #[test]
    fn c_completes_to_connect() {
        let mut i: Input = "/c".into();
        assert!(apply_slash_tab(&mut i, NO_EXT));
        assert_eq!(i.value(), "/connect");
    }

    #[test]
    fn leading_spaces() {
        let mut i = Input::default().with_value("  /con".to_string());
        assert!(apply_slash_tab(&mut i, NO_EXT));
        assert_eq!(i.value(), "  /connect");
    }

    #[test]
    fn candidates_prefix_builtin() {
        assert_eq!(
            candidates_for_prefix("/", NO_EXT),
            vec!["/connect", "/dir", "/prompt-demo"]
        );
        assert_eq!(candidates_for_prefix("/c", NO_EXT), vec!["/connect"]);
        assert_eq!(candidates_for_prefix("/con", NO_EXT), vec!["/connect"]);
        assert_eq!(candidates_for_prefix("/d", NO_EXT), vec!["/dir"]);
        assert_eq!(
            candidates_for_prefix("/p", NO_EXT),
            vec!["/prompt-demo"]
        );
    }

    #[test]
    fn candidates_include_external() {
        let ext = sample_external();
        let all = candidates_for_prefix("/", &ext);
        assert!(all.contains(&"/agent-start".to_string()));
        assert!(all.contains(&"/connect".to_string()));

        let a = candidates_for_prefix("/a", &ext);
        assert_eq!(a, vec!["/agent-start"]);
    }

    #[test]
    fn tab_completes_external() {
        let ext = sample_external();
        let mut i: Input = "/ag".into();
        assert!(apply_slash_tab(&mut i, &ext));
        assert_eq!(i.value(), "/agent-start");
    }

    #[test]
    fn apply_pick() {
        let mut i: Input = "/co".into();
        apply_slash_pick(&mut i, "/connect");
        assert_eq!(i.value(), "/connect");
    }

    #[test]
    fn summaries_builtin() {
        assert_eq!(
            summary_for_command("/connect", NO_EXT),
            Some("打开 provider / model 选择窗口并连接当前后端。".to_string())
        );
        assert_eq!(summary_for_command("/missing", NO_EXT), None);
    }

    #[test]
    fn summaries_external() {
        let ext = sample_external();
        assert_eq!(
            summary_for_command("/agent-start", &ext),
            Some("Start an agent project".to_string())
        );
    }
}
