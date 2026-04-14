use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme::Theme;

pub fn render_markdown(text: &str, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_language = String::new();
    let mut show_code_language_label = false;

    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        let trimmed_start = raw_line.trim_start();

        if trimmed_start.starts_with("```") {
            if in_code_block {
                in_code_block = false;
                code_language.clear();
                show_code_language_label = false;
            } else {
                in_code_block = true;
                code_language = trimmed_start.trim_start_matches("```").trim().to_string();
                show_code_language_label = !code_language.is_empty();
            }
            continue;
        }

        if in_code_block {
            if show_code_language_label {
                let label_style = Style::default().fg(theme.muted).bg(theme.code_bg);
                lines.push(Line::from(vec![Span::styled(
                    format!("  {} ", code_language),
                    label_style,
                )]));
                show_code_language_label = false;
            }

            let code_style = Style::default().fg(theme.code_fg).bg(theme.code_bg);
            lines.push(Line::from(vec![Span::styled(
                format!("  {raw_line}"),
                code_style,
            )]));
            continue;
        }

        if trimmed.is_empty() {
            lines.push(Line::from(String::new()));
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("### ") {
            let style = Style::default()
                .fg(theme.secondary)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(content.to_string(), style)]));
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("## ") {
            let style = Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(content.to_string(), style)]));
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("# ") {
            let style = Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(content.to_string(), style)]));
            continue;
        }

        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let hr_len = usize::max(1, width as usize);
            let style = Style::default().fg(theme.muted).bg(Color::Reset);
            lines.push(Line::from(vec![Span::styled("─".repeat(hr_len), style)]));
            continue;
        }

        if let Some(content) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
        {
            let mut spans = vec![Span::styled(
                "  • ".to_string(),
                Style::default().fg(theme.secondary).bg(theme.background),
            )];
            spans.extend(parse_inline(content, theme).spans);
            lines.push(Line::from(spans));
            continue;
        }

        if let Some((prefix, content)) = parse_numbered_prefix(trimmed_start) {
            let mut spans = vec![Span::styled(
                format!("  {prefix} "),
                Style::default().fg(theme.secondary).bg(theme.background),
            )];
            spans.extend(parse_inline(content, theme).spans);
            lines.push(Line::from(spans));
            continue;
        }

        lines.push(parse_inline(raw_line, theme));
    }

    lines
}

fn parse_inline(line: &str, theme: &Theme) -> Line<'static> {
    if line.is_empty() {
        return Line::from(String::new());
    }

    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buffer = String::new();

    let mut in_bold = false;
    let mut in_italic = false;
    let mut in_inline_code = false;

    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        if ch == '`' {
            if in_inline_code {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_inline_code = false;
                i += 1;
                continue;
            }

            if chars[i + 1..].contains(&'`') {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_inline_code = true;
                i += 1;
                continue;
            }

            buffer.push(ch);
            i += 1;
            continue;
        }

        if !in_inline_code && ch == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if in_bold {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_bold = false;
                i += 2;
                continue;
            }

            if has_closing_double_asterisk(&chars, i + 2) {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_bold = true;
                i += 2;
                continue;
            }

            buffer.push('*');
            buffer.push('*');
            i += 2;
            continue;
        }

        if !in_inline_code && ch == '*' {
            if in_italic {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_italic = false;
                i += 1;
                continue;
            }

            if has_closing_single_asterisk(&chars, i + 1) {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_italic = true;
                i += 1;
                continue;
            }

            buffer.push('*');
            i += 1;
            continue;
        }

        buffer.push(ch);
        i += 1;
    }

    push_buffer(
        &mut spans,
        &mut buffer,
        current_inline_style(theme, in_bold, in_italic, in_inline_code),
    );

    if spans.is_empty() {
        return Line::from(String::new());
    }

    Line::from(spans)
}

fn parse_numbered_prefix(line: &str) -> Option<(&str, &str)> {
    let mut split_idx = 0;
    for (idx, ch) in line.char_indices() {
        if ch.is_ascii_digit() {
            split_idx = idx + ch.len_utf8();
            continue;
        }
        break;
    }

    if split_idx == 0 {
        return None;
    }

    let rest = &line[split_idx..];
    if !rest.starts_with(". ") {
        return None;
    }

    let prefix = &line[..split_idx + 1];
    let content = &rest[2..];
    Some((prefix, content))
}

fn has_closing_double_asterisk(chars: &[char], start: usize) -> bool {
    let mut i = start;
    while i + 1 < chars.len() {
        if chars[i] == '*' && chars[i + 1] == '*' {
            return true;
        }
        i += 1;
    }
    false
}

fn has_closing_single_asterisk(chars: &[char], start: usize) -> bool {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '*' {
            let prev_is_star = i > 0 && chars[i - 1] == '*';
            let next_is_star = i + 1 < chars.len() && chars[i + 1] == '*';
            if !prev_is_star && !next_is_star {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn current_inline_style(
    theme: &Theme,
    in_bold: bool,
    in_italic: bool,
    in_inline_code: bool,
) -> Style {
    let mut style = if in_inline_code {
        Style::default().fg(theme.code_fg).bg(theme.code_bg)
    } else {
        Style::default().fg(theme.foreground).bg(theme.background)
    };

    if !in_inline_code {
        if in_bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if in_italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
    }

    style
}

fn push_buffer(spans: &mut Vec<Span<'static>>, buffer: &mut String, style: Style) {
    if buffer.is_empty() {
        return;
    }

    spans.push(Span::styled(std::mem::take(buffer), style));
}
