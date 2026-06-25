use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::theme::Theme;
use super::utils::sanitize_terminal_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
struct MarkdownTable {
    header: Vec<String>,
    aligns: Vec<TableAlign>,
    rows: Vec<Vec<String>>,
}

pub fn render_markdown(text: &str, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_language = String::new();
    let mut show_code_language_label = false;
    let raw_lines: Vec<&str> = text.lines().collect();

    let mut line_index = 0;
    while line_index < raw_lines.len() {
        let raw_line = raw_lines[line_index];
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
            line_index += 1;
            continue;
        }

        if in_code_block {
            if show_code_language_label {
                let label_style = Style::default().fg(theme.muted).bg(theme.code_bg);
                lines.push(Line::from(vec![Span::styled(
                    sanitize_terminal_text(&format!("  {} ", code_language)),
                    label_style,
                )]));
                show_code_language_label = false;
            }

            let code_style = Style::default().fg(theme.code_fg).bg(theme.code_bg);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(&format!("  {raw_line}")),
                code_style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some((table, consumed)) = parse_table_block(&raw_lines[line_index..]) {
            lines.extend(render_table(&table, theme, width));
            line_index += consumed;
            continue;
        }

        if trimmed.is_empty() {
            lines.push(Line::from(String::new()));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("### ") {
            let style = Style::default()
                .fg(theme.secondary)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(content),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("## ") {
            let style = Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(content),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("# ") {
            let style = Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(content),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let hr_len = usize::max(1, width as usize);
            let style = Style::default().fg(theme.muted).bg(Color::Reset);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(&"─".repeat(hr_len)),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
        {
            let mut spans = vec![Span::styled(
                sanitize_terminal_text("  • "),
                Style::default().fg(theme.secondary).bg(theme.background),
            )];
            spans.extend(parse_inline(content, theme).spans);
            lines.push(Line::from(spans));
            line_index += 1;
            continue;
        }

        if let Some((prefix, content)) = parse_numbered_prefix(trimmed_start) {
            let mut spans = vec![Span::styled(
                format!("  {prefix} "),
                Style::default().fg(theme.secondary).bg(theme.background),
            )];
            spans.extend(parse_inline(content, theme).spans);
            lines.push(Line::from(spans));
            line_index += 1;
            continue;
        }

        lines.push(parse_inline(raw_line, theme));
        line_index += 1;
    }

    lines
}

pub fn contains_markdown_table(text: &str) -> bool {
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut in_code_block = false;
    let mut line_index = 0;

    while line_index < raw_lines.len() {
        let trimmed_start = raw_lines[line_index].trim_start();
        if trimmed_start.starts_with("```") {
            in_code_block = !in_code_block;
            line_index += 1;
            continue;
        }

        if !in_code_block && parse_table_block(&raw_lines[line_index..]).is_some() {
            return true;
        }
        line_index += 1;
    }

    false
}

fn parse_table_block(lines: &[&str]) -> Option<(MarkdownTable, usize)> {
    if lines.len() < 2 {
        return None;
    }

    let header = split_table_cells(lines[0])?;
    let aligns = parse_table_separator(lines[1])?;
    if header.len() < 2 || aligns.len() != header.len() {
        return None;
    }

    let mut rows = Vec::new();
    let mut consumed = 2;
    while consumed < lines.len() {
        let Some(cells) = split_table_cells(lines[consumed]) else {
            break;
        };
        if cells.len() != aligns.len() {
            break;
        }
        rows.push(cells);
        consumed += 1;
    }

    Some((
        MarkdownTable {
            header,
            aligns,
            rows,
        },
        consumed,
    ))
}

fn parse_table_separator(line: &str) -> Option<Vec<TableAlign>> {
    let cells = split_table_cells(line)?;
    if cells.len() < 2 {
        return None;
    }

    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let trimmed = cell.trim();
        if trimmed.len() < 3 {
            return None;
        }
        if !trimmed
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        {
            return None;
        }
        let dash_count = trimmed.chars().filter(|ch| *ch == '-').count();
        if dash_count < 3 {
            return None;
        }

        let starts = trimmed.starts_with(':');
        let ends = trimmed.ends_with(':');
        aligns.push(match (starts, ends) {
            (true, true) => TableAlign::Center,
            (false, true) => TableAlign::Right,
            _ => TableAlign::Left,
        });
    }

    Some(aligns)
}

fn split_table_cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut start = 0;
    let mut end = chars.len();
    if chars.first() == Some(&'|') {
        start += 1;
    }
    if end > start && chars[end - 1] == '|' && !is_escaped(&chars, end - 1) {
        end -= 1;
    }

    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut i = start;
    while i < end {
        let ch = chars[i];
        if ch == '\\' && i + 1 < end && chars[i + 1] == '|' {
            cell.push('|');
            i += 2;
            continue;
        }
        if ch == '|' && !is_escaped(&chars, i) {
            cells.push(cell.trim().to_string());
            cell.clear();
            i += 1;
            continue;
        }
        cell.push(ch);
        i += 1;
    }
    cells.push(cell.trim().to_string());

    if cells.len() < 2 {
        None
    } else {
        Some(cells)
    }
}

fn is_escaped(chars: &[char], index: usize) -> bool {
    let mut slash_count = 0;
    let mut current = index;
    while current > 0 {
        current -= 1;
        if chars[current] == '\\' {
            slash_count += 1;
        } else {
            break;
        }
    }
    slash_count % 2 == 1
}

fn render_table(table: &MarkdownTable, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    if table.header.is_empty() {
        return Vec::new();
    }

    let column_count = table.header.len();
    let column_widths = table_column_widths(&table.header, &table.rows, width, theme);
    let border_style = Style::default().fg(theme.muted).bg(theme.background);

    let mut rendered = Vec::new();
    rendered.push(render_table_border(
        "┌",
        "┬",
        "┐",
        &column_widths,
        border_style,
    ));
    rendered.push(render_table_row(
        &table.header,
        &table.aligns,
        &column_widths,
        theme,
        true,
        border_style,
    ));
    rendered.push(render_table_border(
        "├",
        "┼",
        "┤",
        &column_widths,
        border_style,
    ));
    for row in &table.rows {
        if row.len() == column_count {
            rendered.push(render_table_row(
                row,
                &table.aligns,
                &column_widths,
                theme,
                false,
                border_style,
            ));
        }
    }
    rendered.push(render_table_border(
        "└",
        "┴",
        "┘",
        &column_widths,
        border_style,
    ));
    rendered
}

fn table_column_widths(
    header: &[String],
    body: &[Vec<String>],
    width: u16,
    theme: &Theme,
) -> Vec<usize> {
    let column_count = header.len();
    let mut widths = vec![3; column_count];
    for row in std::iter::once(header).chain(body.iter().map(Vec::as_slice)) {
        for (idx, cell) in row.iter().enumerate().take(column_count) {
            widths[idx] = widths[idx].max(inline_display_width(cell, theme).min(30));
        }
    }

    let available_width = width as usize;
    let fixed_width = column_count + 1 + (column_count * 2);
    let max_content_width = available_width.saturating_sub(fixed_width);
    if max_content_width == 0 {
        return vec![1; column_count];
    }

    while widths.iter().sum::<usize>() > max_content_width {
        if let Some((idx, max_width)) = widths
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(_, width)| *width)
        {
            if max_width <= 1 {
                break;
            }
            widths[idx] -= 1;
        } else {
            break;
        }
    }

    widths
}

fn render_table_border(
    left: &str,
    separator: &str,
    right: &str,
    widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut text = String::new();
    text.push_str(left);
    for (idx, width) in widths.iter().enumerate() {
        if idx > 0 {
            text.push_str(separator);
        }
        text.push_str(&"─".repeat(width + 2));
    }
    text.push_str(right);

    Line::from(vec![Span::styled(sanitize_terminal_text(&text), style)])
}

fn render_table_row(
    cells: &[String],
    aligns: &[TableAlign],
    widths: &[usize],
    theme: &Theme,
    is_header: bool,
    border_style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled(sanitize_terminal_text("│"), border_style)];
    for (idx, width) in widths.iter().enumerate() {
        let cell = cells.get(idx).map(String::as_str).unwrap_or_default();
        let mut cell_line = truncate_inline_line(parse_inline(cell, theme), *width);
        let cell_width = line_display_width(&cell_line);
        let extra = width.saturating_sub(cell_width);
        let (left_pad, right_pad) = match aligns.get(idx).copied().unwrap_or(TableAlign::Left) {
            TableAlign::Left => (0, extra),
            TableAlign::Right => (extra, 0),
            TableAlign::Center => (extra / 2, extra - (extra / 2)),
        };

        spans.push(Span::raw(" ".repeat(left_pad + 1)));
        if is_header {
            for span in &mut cell_line.spans {
                span.style = span
                    .style
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD);
            }
        }
        spans.extend(cell_line.spans);
        spans.push(Span::raw(" ".repeat(right_pad + 1)));
        spans.push(Span::styled(sanitize_terminal_text("│"), border_style));
    }

    Line::from(spans)
}

fn inline_display_width(cell: &str, theme: &Theme) -> usize {
    line_display_width(&parse_inline(cell, theme))
}

fn line_display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

fn truncate_inline_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line_display_width(&line) <= width {
        return line;
    }

    if width <= 1 {
        return Line::from(vec![Span::styled("…", Style::default())]);
    }

    let mut spans = Vec::new();
    let mut used = 0;
    let ellipsis_width = display_width("…");

    'outer: for span in line.spans {
        let mut segment = String::new();
        for ch in span.content.chars() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + char_width + ellipsis_width > width {
                if !segment.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut segment), span.style));
                }
                spans.push(Span::styled("…", span.style));
                break 'outer;
            }
            segment.push(ch);
            used += char_width;
        }
        if !segment.is_empty() {
            spans.push(Span::styled(segment, span.style));
        }
    }

    Line::from(spans)
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
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

    spans.push(Span::styled(
        sanitize_terminal_text(&std::mem::take(buffer)),
        style,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        Theme::detect()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn renders_markdown_table_as_terminal_table() {
        let lines = render_markdown(
            "| Name | Status |\n| --- | --- |\n| xiaoO | ready |",
            &test_theme(),
            80,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text.len(), 5);
        assert!(text[0].starts_with('┌'));
        assert!(text[1].contains("Name"));
        assert!(text[1].contains("Status"));
        assert!(text[2].starts_with('├'));
        assert!(text[3].contains("xiaoO"));
        assert!(text[3].contains("ready"));
        assert!(text[4].starts_with('└'));
    }

    #[test]
    fn table_separator_requires_three_dashes() {
        let lines = render_markdown("| A | B |\n| - | - |\n| 1 | 2 |", &test_theme(), 80);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text[0], "| A | B |");
        assert_eq!(text[1], "| - | - |");
        assert_eq!(text[2], "| 1 | 2 |");
    }

    #[test]
    fn table_rendering_truncates_to_available_width() {
        let lines = render_markdown(
            "| Column A | Column B |\n| --- | --- |\n| a very long value | another long value |",
            &test_theme(),
            24,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(text.iter().all(|line| display_width(line) <= 24));
        assert!(text.iter().any(|line| line.contains('…')));
    }

    #[test]
    fn table_alignment_uses_rendered_inline_width() {
        let lines = render_markdown(
            "| 类型 | 名称 | 大小 |\n| --- | --- | ---: |\n| 📁 | `.cargo/` | - |\n| 📄 | `README.md` | 9.1 KB |",
            &test_theme(),
            80,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        let header_columns = vertical_border_columns(&text[1]);
        let first_row_columns = vertical_border_columns(&text[3]);
        let second_row_columns = vertical_border_columns(&text[4]);

        assert_eq!(first_row_columns, header_columns);
        assert_eq!(second_row_columns, header_columns);
    }

    #[test]
    fn detects_markdown_tables_outside_code_blocks() {
        assert!(contains_markdown_table(
            "before\n| A | B |\n| --- | --- |\n| 1 | 2 |"
        ));
        assert!(!contains_markdown_table(
            "```\n| A | B |\n| --- | --- |\n| 1 | 2 |\n```"
        ));
    }

    fn vertical_border_columns(line: &str) -> Vec<usize> {
        let mut columns = Vec::new();
        let mut width = 0;
        for ch in line.chars() {
            if ch == '│' {
                columns.push(width);
            }
            width += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        columns
    }
}
