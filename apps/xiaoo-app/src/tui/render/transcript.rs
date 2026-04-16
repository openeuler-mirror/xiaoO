use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, Wrap},
    Frame,
};

use crate::app::App;
use crate::app_state::ToolToggleRegion;
use crate::chat::{MessageRole, TodoDisplayStatus, ToolExecutionStatus};
use crate::markdown::render_markdown;

use super::utils::{render_tool_detail_text, rendered_line_count, truncate_display_width};

impl App {
    pub(crate) fn render_chat(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .title(" Messages ")
            .style(Style::default().bg(self.state.theme.background));
        let inner_area = block.inner(area);
        let scrollbar_area = Rect {
            x: area.x,
            y: inner_area.y,
            width: area.width,
            height: inner_area.height,
        };
        self.state.render_state.messages_area = Some(scrollbar_area);
        frame.render_widget(block.clone(), area);

        let inner_height = inner_area.height as usize;
        let mut message_entries: Vec<(usize, Vec<Line>, bool)> = Vec::new();
        for (message_index, message) in self.state.chat_state.messages.iter().enumerate() {
            if let Some(tool) = &message.tool_state {
                let tool_color = match tool.status {
                    ToolExecutionStatus::Running => self.state.theme.accent,
                    ToolExecutionStatus::Completed => self.state.theme.success,
                    ToolExecutionStatus::Failed => self.state.theme.error,
                };
                let timestamp = message.timestamp.format("%H:%M:%S").to_string();
                let toggle = if tool.expanded { "▾" } else { "▸" };
                let status = match tool.status {
                    ToolExecutionStatus::Running => "running",
                    ToolExecutionStatus::Completed => "done",
                    ToolExecutionStatus::Failed => "failed",
                };
                let mut header = format!("{} {}  {}", toggle, tool.tool, status);
                if let Some(exit_code) = tool.exit_code {
                    header.push_str(&format!("  exit={}", exit_code));
                }
                if let Some(duration_ms) = tool.duration_ms {
                    header.push_str(&format!("  {}ms", duration_ms));
                }
                if !tool.summary.trim().is_empty() {
                    header.push_str(&format!("  {}", tool.summary.trim()));
                }
                let max_header_width = inner_area.width.saturating_sub(2) as usize;
                let header = truncate_display_width(&header, max_header_width);

                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("▎ ", Style::default().fg(tool_color)),
                        Span::styled(
                            "Tool",
                            Style::default().fg(tool_color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {}", timestamp),
                            Style::default().fg(self.state.theme.muted),
                        ),
                    ]),
                    Line::styled(header, Style::default().fg(tool_color)),
                ];

                let command_text = if tool.expanded {
                    tool.command.as_deref()
                } else {
                    tool.command_preview.as_deref()
                };
                if let Some(command_text) = command_text.filter(|text| !text.trim().is_empty()) {
                    lines.push(Line::styled(
                        "  Command",
                        Style::default()
                            .fg(self.state.theme.muted)
                            .add_modifier(Modifier::BOLD),
                    ));
                    for line in command_text.lines() {
                        lines.push(Line::styled(
                            format!("    {}", line),
                            Style::default().fg(self.state.theme.foreground),
                        ));
                    }
                    if !tool.expanded && tool.command_preview != tool.command {
                        lines.push(Line::styled(
                            "    ... click to expand full command",
                            Style::default()
                                .fg(self.state.theme.muted)
                                .add_modifier(Modifier::ITALIC),
                        ));
                    }
                }

                if tool.expanded && tool.command.is_none() && !tool.args_preview.trim().is_empty() {
                    lines.push(Line::styled(
                        "  Arguments",
                        Style::default()
                            .fg(self.state.theme.muted)
                            .add_modifier(Modifier::BOLD),
                    ));
                    for line in tool.args_preview.lines() {
                        lines.push(Line::styled(
                            format!("    {}", line),
                            Style::default().fg(self.state.theme.foreground),
                        ));
                    }
                }

                let detail_text = render_tool_detail_text(&tool.detail);
                let detail_text = detail_text.trim();
                if tool.expanded && !detail_text.is_empty() {
                    lines.push(Line::styled(
                        "  Output",
                        Style::default()
                            .fg(self.state.theme.muted)
                            .add_modifier(Modifier::BOLD),
                    ));
                    for line in detail_text.lines() {
                        lines.push(Line::styled(
                            format!("    {}", line),
                            Style::default().fg(self.state.theme.foreground),
                        ));
                    }
                }
                lines.push(Line::raw(""));
                message_entries.push((message_index, lines, true));
                continue;
            }

            if let Some(todo) = &message.todo_state {
                let timestamp = message.timestamp.format("%H:%M:%S").to_string();
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("▎ ", Style::default().fg(self.state.theme.secondary)),
                        Span::styled(
                            "Planner",
                            Style::default()
                                .fg(self.state.theme.secondary)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {}", timestamp),
                            Style::default().fg(self.state.theme.muted),
                        ),
                    ]),
                    Line::styled(
                        format!("  {}", todo.title),
                        Style::default()
                            .fg(self.state.theme.secondary)
                            .add_modifier(Modifier::BOLD),
                    ),
                ];

                for (status, content) in &todo.items {
                    let (icon, color) = match status {
                        TodoDisplayStatus::Completed => ("✅", self.state.theme.success),
                        TodoDisplayStatus::InProgress => ("◔", self.state.theme.accent),
                        TodoDisplayStatus::Pending => ("☐", self.state.theme.muted),
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                        Span::styled(
                            content.as_str(),
                            Style::default().fg(self.state.theme.foreground),
                        ),
                    ]));
                }
                lines.push(Line::raw(""));
                message_entries.push((message_index, lines, false));
                continue;
            }

            if let Some(checker) = &message.completion_check_state {
                let timestamp = message.timestamp.format("%H:%M:%S").to_string();
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("▎ ", Style::default().fg(self.state.theme.gradient_yellow)),
                        Span::styled(
                            "Checker",
                            Style::default()
                                .fg(self.state.theme.gradient_yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {}", timestamp),
                            Style::default().fg(self.state.theme.muted),
                        ),
                    ]),
                    Line::styled(
                        "  next_step_hint",
                        Style::default()
                            .fg(self.state.theme.gradient_yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ];

                if !checker.next_step_hint.trim().is_empty() {
                    lines.push(Line::styled(
                        format!("  → {}", checker.next_step_hint.trim()),
                        Style::default().fg(self.state.theme.foreground),
                    ));
                }
                if !checker.missing_information.trim().is_empty() {
                    lines.push(Line::styled(
                        format!(
                            "  missing_information: {}",
                            checker.missing_information.trim()
                        ),
                        Style::default().fg(self.state.theme.muted),
                    ));
                }
                if !checker.reason.trim().is_empty() {
                    lines.push(Line::styled(
                        format!("  reason: {}", checker.reason.trim()),
                        Style::default().fg(self.state.theme.muted),
                    ));
                }
                lines.push(Line::raw(""));
                message_entries.push((message_index, lines, false));
                continue;
            }

            let (indicator_color, role_label, role_style) = match message.role {
                MessageRole::User => (
                    self.state.theme.primary,
                    "You",
                    Style::default()
                        .fg(self.state.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                MessageRole::Assistant => (
                    self.state.theme.accent,
                    "Assistant",
                    Style::default()
                        .fg(self.state.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                MessageRole::System => (
                    self.state.theme.success,
                    "System",
                    Style::default().fg(self.state.theme.success),
                ),
                MessageRole::Tool => (
                    self.state.theme.muted,
                    "Tool",
                    Style::default().fg(self.state.theme.muted),
                ),
            };

            let timestamp = message.timestamp.format("%H:%M:%S").to_string();
            let is_active_stream_message = self.gateway.stream_message_index == Some(message_index);
            let show_stream_thinking = message.role == MessageRole::Assistant
                && message.is_streaming
                && is_active_stream_message
                && message.content.is_empty();
            let mut lines = vec![Line::from(vec![
                Span::styled("▎ ", Style::default().fg(indicator_color)),
                Span::styled(role_label.to_string(), role_style),
                Span::styled(
                    format!("  {}", timestamp),
                    Style::default().fg(self.state.theme.muted),
                ),
            ])];

            if !message.thinking_content.is_empty() {
                let is_thinking = self.state.chat_state.is_loading
                    && is_active_stream_message
                    && message.content.is_empty();
                let thinking_header = if is_thinking {
                    format!("  ⟡ {}", self.loading_animation())
                } else {
                    "  ⟡ Thought".to_string()
                };
                lines.push(Line::styled(
                    thinking_header,
                    Style::default()
                        .fg(self.state.theme.muted)
                        .add_modifier(Modifier::ITALIC),
                ));
                let thinking_style = Style::default()
                    .fg(self.state.theme.muted)
                    .add_modifier(Modifier::DIM);
                for line in message.thinking_content.lines() {
                    lines.push(Line::styled(format!("  │ {}", line), thinking_style));
                }
                lines.push(Line::raw(""));
            }

            if show_stream_thinking {
                lines.push(Line::styled(
                    format!("  {}", self.loading_animation()),
                    Style::default().fg(self.state.theme.accent),
                ));
            }

            match message.role {
                MessageRole::Assistant if !message.content.is_empty() => {
                    let markdown_lines =
                        render_markdown(&message.content, &self.state.theme, inner_area.width);
                    lines.extend(markdown_lines);
                }
                _ => {
                    for line in message.content.lines() {
                        lines.push(Line::styled(
                            format!("  {}", line),
                            Style::default().fg(self.state.theme.foreground),
                        ));
                    }
                }
            }

            if message.is_streaming && !show_stream_thinking {
                lines.push(Line::styled(
                    "  ▌",
                    Style::default().fg(self.state.theme.accent),
                ));
            }
            lines.push(Line::raw(""));
            message_entries.push((message_index, lines, false));
        }

        let mut all_lines: Vec<Line> = Vec::new();
        let mut line_is_header: Vec<bool> = Vec::new();
        for (_, lines, _) in &message_entries {
            for (i, line) in lines.iter().enumerate() {
                all_lines.push(line.clone());
                // The first line of every entry is the "▎ Role  HH:MM:SS" header.
                // Mark it so that selection extraction can skip it.
                line_is_header.push(i == 0);
            }
        }

        // Cache the plain-text content of every rendered line so that
        // transcript selection can extract text without re-rendering.
        self.state.render_state.line_texts = all_lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        self.state.render_state.line_is_header = line_is_header;

        // Apply selection highlighting to the line copies used for rendering.
        if let Some(sel) = &self.state.transcript_selection {
            let (start_line, start_col, end_line, end_col) = sel.normalised();
            let sel_style = Style::default()
                .fg(self.state.theme.background)
                .bg(self.state.theme.foreground)
                .add_modifier(Modifier::BOLD);
            for (line_idx, line) in all_lines.iter_mut().enumerate() {
                if line_idx < start_line || line_idx > end_line {
                    continue;
                }
                let col_start = if line_idx == start_line { start_col } else { 0 };
                let line_char_len: usize =
                    line.spans.iter().map(|s| s.content.chars().count()).sum();
                let col_end = if line_idx == end_line {
                    end_col.min(line_char_len)
                } else {
                    line_char_len
                };
                if col_start >= col_end {
                    continue;
                }
                *line = highlight_line_selection(line.clone(), col_start, col_end, sel_style);
            }
        }

        let total_lines = rendered_line_count(&all_lines, inner_area.width);
        let content = Text::from(all_lines);
        let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
        self.state.chat_state.total_lines = total_lines;
        self.state.chat_state.last_visible_height = inner_height;

        let max_scroll = total_lines.saturating_sub(inner_height).min(total_lines);
        if self.state.chat_state.stick_to_bottom {
            self.state.chat_state.scroll_offset = max_scroll;
        } else {
            self.state.chat_state.scroll_offset =
                self.state.chat_state.scroll_offset.min(max_scroll);
        }
        let scroll_offset = self.state.chat_state.scroll_offset;

        self.state.render_state.tool_toggle_regions.clear();
        let mut absolute_row = 0usize;
        for (message_index, lines, toggleable) in &message_entries {
            let toggle_row = absolute_row.saturating_add(1);
            if *toggleable
                && toggle_row >= scroll_offset
                && toggle_row < scroll_offset.saturating_add(inner_height)
            {
                self.state
                    .render_state
                    .tool_toggle_regions
                    .push(ToolToggleRegion {
                        message_index: *message_index,
                        rect: Rect {
                            x: inner_area.x,
                            y: inner_area.y + (toggle_row.saturating_sub(scroll_offset) as u16),
                            width: inner_area.width,
                            height: 1,
                        },
                    });
            }
            absolute_row += rendered_line_count(lines, inner_area.width);
        }

        let paragraph = paragraph.scroll((scroll_offset as u16, 0));
        frame.render_widget(paragraph, inner_area);

        self.state.chat_state.scrollbar_state = self
            .state
            .chat_state
            .scrollbar_state
            .content_length(total_lines)
            .viewport_content_length(inner_height)
            .position(scroll_offset);

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .style(Style::default().fg(self.state.theme.border));
        frame.render_stateful_widget(
            scrollbar,
            scrollbar_area,
            &mut self.state.chat_state.scrollbar_state,
        );
    }
}

/// Restyle the characters in `col_start..col_end` (char indices) within a
/// ratatui `Line` that may contain multiple spans.  Characters outside the
/// range keep their original style.
fn highlight_line_selection(
    line: Line<'_>,
    col_start: usize,
    col_end: usize,
    sel_style: Style,
) -> Line<'_> {
    let mut new_spans: Vec<Span<'_>> = Vec::new();
    let mut char_offset: usize = 0;

    for span in line.spans {
        let span_len = span.content.chars().count();
        let span_end = char_offset + span_len;

        let ov_start = col_start.max(char_offset);
        let ov_end = col_end.min(span_end);

        if ov_start >= ov_end {
            // No overlap – keep span as-is.
            new_spans.push(span.clone());
        } else {
            let local_start = ov_start - char_offset;
            let local_end = ov_end - char_offset;

            let before: String = span.content.chars().take(local_start).collect();
            let selected: String = span
                .content
                .chars()
                .skip(local_start)
                .take(local_end - local_start)
                .collect();
            let after: String = span.content.chars().skip(local_end).collect();

            if !before.is_empty() {
                new_spans.push(Span::styled(before, span.style));
            }
            if !selected.is_empty() {
                new_spans.push(Span::styled(selected, sel_style));
            }
            if !after.is_empty() {
                new_spans.push(Span::styled(after, span.style));
            }
        }

        char_offset = span_end;
    }

    Line::from(new_spans)
}
