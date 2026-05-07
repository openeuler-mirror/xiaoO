use chrono::TimeZone;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation},
    Frame,
};
use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::app_state::{
    CachedMessageLayout, CachedMessageRender, ToolToggleRegion, TranscriptRenderCache,
};
use crate::chat::{Message, MessageRole, ToolExecutionStatus, ToolMessageState};
use crate::markdown::render_markdown;
use crate::theme::Theme;

use super::utils::{
    render_tool_detail_text, rendered_line_count, sanitize_terminal_text, truncate_display_width,
};

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
        let loading_animation = self.loading_animation();
        let message_count = self.state.chat_state.messages.len();
        if self.state.render_state.message_renders.len() != message_count {
            self.state
                .render_state
                .message_renders
                .resize(message_count, None);
            self.state.render_state.transcript_cache = None;
        }

        let mut transcript_dirty = self.state.render_state.transcript_cache.is_none();
        for message_index in 0..message_count {
            let message = &self.state.chat_state.messages[message_index];
            let is_active_stream_message = self.gateway.stream_message_index == Some(message_index);
            let should_bypass_cache = is_active_stream_message && self.state.chat_state.is_loading;
            if should_bypass_cache {
                transcript_dirty = true;
                continue;
            }

            let cache_slot = &mut self.state.render_state.message_renders[message_index];
            let needs_rebuild = cache_slot.as_ref().is_none_or(|cached| {
                cached.revision != message.render_revision
                    || cached.width != inner_area.width
                    || cached.theme != self.state.theme
            });
            if needs_rebuild {
                *cache_slot = Some(render_message_entry(
                    message,
                    &self.state.theme,
                    inner_area.width,
                    is_active_stream_message,
                    self.state.chat_state.is_loading,
                    &loading_animation,
                ));
                transcript_dirty = true;
            }
        }

        if transcript_dirty {
            let mut current_renders = Vec::with_capacity(message_count);
            for message_index in 0..message_count {
                let message = &self.state.chat_state.messages[message_index];
                let is_active_stream_message =
                    self.gateway.stream_message_index == Some(message_index);
                let should_bypass_cache =
                    is_active_stream_message && self.state.chat_state.is_loading;
                if should_bypass_cache {
                    current_renders.push(render_message_entry(
                        message,
                        &self.state.theme,
                        inner_area.width,
                        is_active_stream_message,
                        self.state.chat_state.is_loading,
                        &loading_animation,
                    ));
                } else {
                    current_renders.push(
                        self.state.render_state.message_renders[message_index]
                            .as_ref()
                            .expect("message render cache must be populated")
                            .clone(),
                    );
                }
            }
            let transcript_cache = build_transcript_cache(&current_renders);
            self.state.render_state.line_texts = transcript_cache.line_texts.clone();
            self.state.render_state.line_is_header = transcript_cache.line_is_header.clone();
            self.state.render_state.transcript_cache = Some(transcript_cache);
        }

        let transcript_cache = self
            .state
            .render_state
            .transcript_cache
            .as_ref()
            .expect("transcript cache must be populated");

        self.state.chat_state.total_lines = transcript_cache.total_lines;
        self.state.chat_state.last_visible_height = inner_height;

        let max_scroll = transcript_cache
            .total_lines
            .saturating_sub(inner_height)
            .min(transcript_cache.total_lines);
        if self.state.chat_state.stick_to_bottom {
            self.state.chat_state.scroll_offset = max_scroll;
        } else {
            self.state.chat_state.scroll_offset =
                self.state.chat_state.scroll_offset.min(max_scroll);
        }
        let scroll_offset = self.state.chat_state.scroll_offset;
        let scroll_end = scroll_offset.saturating_add(inner_height);
        if let Some(sel) = &self.state.transcript_selection {
            let start_line_index = transcript_cache
                .logical_line_visual_starts
                .partition_point(|start| *start <= scroll_offset)
                .saturating_sub(1);
            let safe_start_line_index =
                start_line_index.min(transcript_cache.all_lines.len().saturating_sub(1));
            let slice_start_visual = transcript_cache
                .logical_line_visual_starts
                .get(safe_start_line_index)
                .copied()
                .unwrap_or(0);
            let paragraph_scroll = scroll_offset.saturating_sub(slice_start_visual);

            let mut end_line_index = safe_start_line_index;
            while end_line_index < transcript_cache.all_lines.len() {
                let line_start = transcript_cache.logical_line_visual_starts[end_line_index];
                if line_start >= scroll_end {
                    break;
                }
                end_line_index += 1;
            }
            if end_line_index == safe_start_line_index
                && end_line_index < transcript_cache.all_lines.len()
            {
                end_line_index += 1;
            }

            let (start_line, start_col, end_line, end_col) = sel.normalised();
            let sel_style = Style::default()
                .fg(self.state.theme.background)
                .bg(self.state.theme.foreground)
                .add_modifier(Modifier::BOLD);
            let mut selected_visual_lines = Vec::new();
            for (visible_index, original_line) in transcript_cache.all_lines
                [safe_start_line_index..end_line_index]
                .iter()
                .enumerate()
            {
                let global_line_index = safe_start_line_index + visible_index;
                let line = if global_line_index < start_line || global_line_index > end_line {
                    original_line.clone()
                } else {
                    let col_start = if global_line_index == start_line {
                        start_col
                    } else {
                        0
                    };
                    let line_char_len: usize = original_line
                        .spans
                        .iter()
                        .map(|span| span.content.chars().count())
                        .sum();
                    let col_end = if global_line_index == end_line {
                        end_col.min(line_char_len)
                    } else {
                        line_char_len
                    };
                    if col_start >= col_end {
                        original_line.clone()
                    } else {
                        highlight_line_selection(
                            original_line.clone(),
                            col_start,
                            col_end,
                            sel_style,
                        )
                    }
                };
                selected_visual_lines.extend(wrap_line_to_visual_lines(&line, inner_area.width));
            }

            let visual_slice_start = paragraph_scroll.min(selected_visual_lines.len());
            let visual_slice_end = visual_slice_start
                .saturating_add(inner_height)
                .min(selected_visual_lines.len());
            let visible_visual_lines = if visual_slice_start < visual_slice_end {
                selected_visual_lines[visual_slice_start..visual_slice_end].to_vec()
            } else {
                Vec::new()
            };

            let paragraph = Paragraph::new(Text::from(visible_visual_lines));
            frame.render_widget(paragraph, inner_area);
        } else {
            let visual_end = scroll_end.min(transcript_cache.visual_lines.len());
            let visible_visual_lines = if scroll_offset < visual_end {
                transcript_cache.visual_lines[scroll_offset..visual_end].to_vec()
            } else {
                Vec::new()
            };
            let paragraph = Paragraph::new(Text::from(visible_visual_lines));
            frame.render_widget(paragraph, inner_area);
        }

        self.state.render_state.tool_toggle_regions.clear();
        for layout in &transcript_cache.message_layouts {
            if let Some(toggle_row_offset) = layout.tool_toggle_row_offset {
                let toggle_row = layout.start_visual_row.saturating_add(toggle_row_offset);
                if toggle_row >= scroll_offset && toggle_row < scroll_end {
                    self.state
                        .render_state
                        .tool_toggle_regions
                        .push(ToolToggleRegion {
                            message_index: layout.message_index,
                            rect: Rect {
                                x: inner_area.x,
                                y: inner_area.y + (toggle_row.saturating_sub(scroll_offset) as u16),
                                width: inner_area.width,
                                height: 1,
                            },
                        });
                }
            }
        }

        self.state.chat_state.scrollbar_state = self
            .state
            .chat_state
            .scrollbar_state
            .content_length(transcript_cache.total_lines)
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

fn render_message_entry(
    message: &Message,
    theme: &Theme,
    width: u16,
    is_active_stream_message: bool,
    chat_is_loading: bool,
    loading_animation: &str,
) -> CachedMessageRender {
    let mut tool_toggle_row_offset = None;

    let lines = if let Some(tool) = &message.tool_state {
        let tool_color = match tool.status {
            ToolExecutionStatus::Running => theme.accent,
            ToolExecutionStatus::Completed => theme.success,
            ToolExecutionStatus::Failed => theme.error,
        };
        let timestamp = message.timestamp.format("%H:%M:%S").to_string();
        if is_subagent_tool(&tool.tool) {
            tool_toggle_row_offset = Some(0);
            let mut lines = render_subagent_tool_lines(tool, &timestamp, tool_color, theme, width);
            lines.push(Line::raw(""));
            lines
        } else {
            tool_toggle_row_offset = Some(1);
            render_tool_message_lines(message, tool, tool_color, theme, width)
        }
    } else if let Some(checker) = &message.completion_check_state {
        render_completion_check_lines(message, checker, theme)
    } else {
        render_standard_message_lines(
            message,
            theme,
            width,
            is_active_stream_message,
            chat_is_loading,
            loading_animation,
        )
    };

    CachedMessageRender {
        revision: message.render_revision,
        width,
        theme: *theme,
        tool_toggle_row_offset,
        lines,
    }
}

fn build_transcript_cache(message_renders: &[CachedMessageRender]) -> TranscriptRenderCache {
    let mut all_lines = Vec::new();
    let mut visual_lines = Vec::new();
    let mut line_texts = Vec::new();
    let mut line_is_header = Vec::new();
    let mut logical_line_visual_starts = Vec::new();
    let mut message_layouts = Vec::with_capacity(message_renders.len());
    let mut absolute_visual_row = 0usize;

    for (message_index, render) in message_renders.iter().enumerate() {
        message_layouts.push(CachedMessageLayout {
            message_index,
            start_visual_row: absolute_visual_row,
            tool_toggle_row_offset: render.tool_toggle_row_offset,
        });

        for (line_index, line) in render.lines.iter().enumerate() {
            let visual_count = rendered_line_count(std::slice::from_ref(line), render.width);
            logical_line_visual_starts.push(absolute_visual_row);
            absolute_visual_row += visual_count;
            visual_lines.extend(wrap_line_to_visual_lines(line, render.width));

            line_texts.push(
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
            );
            line_is_header.push(line_index == 0);
            all_lines.push(line.clone());
        }
    }

    TranscriptRenderCache {
        all_lines,
        visual_lines,
        line_texts,
        line_is_header,
        logical_line_visual_starts,
        message_layouts,
        total_lines: absolute_visual_row,
    }
}

fn wrap_line_to_visual_lines(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    if line.spans.is_empty() {
        return vec![preserve_line_metadata(Line::from(String::new()), line)];
    }

    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for span in &line.spans {
        let style = span.style;
        let mut segment = String::new();

        for ch in span.content.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if current_width > 0 && current_width + ch_width > width {
                if !segment.is_empty() {
                    current_spans.push(Span::styled(std::mem::take(&mut segment), style));
                }
                rows.push(preserve_line_metadata(
                    Line::from(std::mem::take(&mut current_spans)),
                    line,
                ));
                current_width = 0;
            }

            segment.push(ch);
            current_width += ch_width;

            if current_width == width {
                if !segment.is_empty() {
                    current_spans.push(Span::styled(std::mem::take(&mut segment), style));
                }
                rows.push(preserve_line_metadata(
                    Line::from(std::mem::take(&mut current_spans)),
                    line,
                ));
                current_width = 0;
            }
        }

        if !segment.is_empty() {
            current_spans.push(Span::styled(segment, style));
        }
    }

    if !current_spans.is_empty() || rows.is_empty() {
        rows.push(preserve_line_metadata(Line::from(current_spans), line));
    }

    rows
}

fn preserve_line_metadata(mut rebuilt: Line<'static>, original: &Line<'static>) -> Line<'static> {
    rebuilt.style = original.style;
    rebuilt.alignment = original.alignment;
    rebuilt
}

fn render_tool_message_lines(
    message: &Message,
    tool: &ToolMessageState,
    tool_color: ratatui::style::Color,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let timestamp = message.timestamp.format("%H:%M:%S").to_string();
    let toggle = sanitize_terminal_text(if tool.expanded { "▾" } else { "▸" });
    let status = match tool.status {
        ToolExecutionStatus::Running => "running",
        ToolExecutionStatus::Completed => "done",
        ToolExecutionStatus::Failed => "failed",
    };
    let mut header = format!("{toggle} {}  {status}", tool.tool);
    if let Some(exit_code) = tool.exit_code {
        header.push_str(&format!("  exit={exit_code}"));
    }
    if let Some(duration_ms) = tool.duration_ms {
        header.push_str(&format!("  {duration_ms}ms"));
    }
    if !tool.summary.trim().is_empty() {
        header.push_str(&format!("  {}", tool.summary.trim()));
    }
    let max_header_width = width.saturating_sub(2) as usize;
    let header = truncate_display_width(&header, max_header_width);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                sanitize_terminal_text("▎ "),
                Style::default().fg(tool_color),
            ),
            Span::styled(
                "Tool",
                Style::default().fg(tool_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {timestamp}"), Style::default().fg(theme.muted)),
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
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        for line in command_text.lines() {
            lines.push(Line::styled(
                format!("    {}", sanitize_terminal_text(line)),
                Style::default().fg(theme.foreground),
            ));
        }
        if !tool.expanded && tool.command_preview != tool.command {
            lines.push(Line::styled(
                "    ... click to expand full command",
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
    }

    if tool.expanded && tool.command.is_none() && !tool.args_preview.trim().is_empty() {
        lines.push(Line::styled(
            "  Arguments",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        for line in tool.args_preview.lines() {
            lines.push(Line::styled(
                format!("    {}", sanitize_terminal_text(line)),
                Style::default().fg(theme.foreground),
            ));
        }
    }

    let detail_text = render_tool_detail_text(&tool.detail);
    let detail_text = detail_text.trim();
    if tool.expanded && !detail_text.is_empty() {
        lines.push(Line::styled(
            "  Output",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        for line in detail_text.lines() {
            lines.push(Line::styled(
                format!("    {}", sanitize_terminal_text(line)),
                Style::default().fg(theme.foreground),
            ));
        }
    }
    lines.push(Line::raw(""));
    lines
}

fn render_completion_check_lines(
    message: &Message,
    checker: &crate::chat::CompletionCheckMessageState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let timestamp = message.timestamp.format("%H:%M:%S").to_string();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                sanitize_terminal_text("▎ "),
                Style::default().fg(theme.gradient_yellow),
            ),
            Span::styled(
                "Checker",
                Style::default()
                    .fg(theme.gradient_yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {timestamp}"), Style::default().fg(theme.muted)),
        ]),
        Line::styled(
            "  next_step_hint",
            Style::default()
                .fg(theme.gradient_yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    if !checker.next_step_hint.trim().is_empty() {
        lines.push(Line::styled(
            format!(
                "  {} {}",
                sanitize_terminal_text("→"),
                sanitize_terminal_text(checker.next_step_hint.trim())
            ),
            Style::default().fg(theme.foreground),
        ));
    }
    if !checker.missing_information.trim().is_empty() {
        lines.push(Line::styled(
            format!(
                "  missing_information: {}",
                checker.missing_information.trim()
            ),
            Style::default().fg(theme.muted),
        ));
    }
    if !checker.reason.trim().is_empty() {
        lines.push(Line::styled(
            format!("  reason: {}", checker.reason.trim()),
            Style::default().fg(theme.muted),
        ));
    }
    lines.push(Line::raw(""));
    lines
}

fn render_standard_message_lines(
    message: &Message,
    theme: &Theme,
    width: u16,
    is_active_stream_message: bool,
    chat_is_loading: bool,
    loading_animation: &str,
) -> Vec<Line<'static>> {
    let (indicator_color, role_label, role_style, content_style) = match message.role {
        MessageRole::User => (
            theme.primary,
            "You",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(theme.foreground),
        ),
        MessageRole::Assistant => (
            theme.accent,
            "Assistant",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(theme.foreground),
        ),
        MessageRole::System => (
            theme.success,
            "System",
            Style::default().fg(theme.success),
            Style::default().fg(theme.foreground),
        ),
        MessageRole::Error => (
            theme.error,
            "Error",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(theme.error),
        ),
        MessageRole::Tool => (
            theme.muted,
            "Tool",
            Style::default().fg(theme.muted),
            Style::default().fg(theme.foreground),
        ),
    };

    let timestamp = message.timestamp.format("%H:%M:%S").to_string();
    let show_stream_thinking = message.role == MessageRole::Assistant
        && message.is_streaming
        && is_active_stream_message
        && message.content.is_empty();
    let mut lines = vec![Line::from(vec![
        Span::styled(
            sanitize_terminal_text("▎ "),
            Style::default().fg(indicator_color),
        ),
        Span::styled(role_label.to_string(), role_style),
        Span::styled(format!("  {timestamp}"), Style::default().fg(theme.muted)),
    ])];

    if !message.thinking_content.is_empty() {
        let is_thinking = chat_is_loading && is_active_stream_message && message.content.is_empty();
        let thinking_header = if is_thinking {
            format!("  {} {loading_animation}", sanitize_terminal_text("⭕️"))
        } else {
            format!("  {} Thought", sanitize_terminal_text("⭕️"))
        };
        lines.push(Line::styled(
            thinking_header,
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        ));
        let thinking_style = Style::default().fg(theme.muted).add_modifier(Modifier::DIM);
        for line in message.thinking_content.lines() {
            lines.push(Line::styled(
                format!(
                    "  {} {}",
                    sanitize_terminal_text("│"),
                    sanitize_terminal_text(line)
                ),
                thinking_style,
            ));
        }
        lines.push(Line::raw(""));
    }

    if show_stream_thinking {
        lines.push(Line::styled(
            format!("  {loading_animation}"),
            Style::default().fg(theme.accent),
        ));
    }

    match message.role {
        MessageRole::Assistant if !message.content.is_empty() => {
            lines.extend(render_markdown(&message.content, theme, width));
        }
        _ => {
            for line in message.content.lines() {
                lines.push(Line::styled(
                    format!("  {}", sanitize_terminal_text(line)),
                    content_style,
                ));
            }
        }
    }

    if message.is_streaming && !show_stream_thinking {
        lines.push(Line::styled(
            format!("  {}", sanitize_terminal_text("▌")),
            Style::default().fg(theme.accent),
        ));
    }
    lines.push(Line::raw(""));
    lines
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

    let mut rebuilt = Line::from(new_spans);
    rebuilt.style = line.style;
    rebuilt.alignment = line.alignment;
    rebuilt
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JoinSubagentTerminalDetail {
    status: String,
    reply: Option<String>,
    error: Option<String>,
    completed_at_ms: Option<u64>,
}

fn is_subagent_tool(tool_name: &str) -> bool {
    matches!(tool_name, "spawn_subagent" | "join_subagent")
}

fn render_subagent_tool_lines(
    tool: &ToolMessageState,
    timestamp: &str,
    tool_color: ratatui::style::Color,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let title = match tool.tool.as_str() {
        "spawn_subagent" => "Spawn Subagent",
        "join_subagent" => "Join Subagent",
        _ => "Subagent",
    };
    let toggle = sanitize_terminal_text(if tool.expanded { "▾" } else { "▸" });
    let status = match tool.status {
        ToolExecutionStatus::Running => "running",
        ToolExecutionStatus::Completed => "done",
        ToolExecutionStatus::Failed => "failed",
    };
    let hint = if tool.expanded {
        "click to collapse"
    } else {
        "click to expand details"
    };
    let mut header = format!("{toggle} {title}  {status}  {timestamp}  {hint}");
    if let Some(duration_ms) = tool.duration_ms {
        header.push_str(&format!("  {}ms", duration_ms));
    }
    let max_header_width = width.saturating_sub(2) as usize;
    let header = truncate_display_width(&header, max_header_width);

    let mut lines = vec![Line::from(vec![
        Span::styled(
            sanitize_terminal_text("▎ "),
            Style::default().fg(tool_color),
        ),
        Span::styled(
            header,
            Style::default().fg(tool_color).add_modifier(Modifier::BOLD),
        ),
    ])];

    if !tool.expanded {
        return lines;
    }

    if !tool.args_preview.trim().is_empty() {
        lines.push(Line::styled(
            "  Input JSON",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        for line in tool.args_preview.lines() {
            lines.push(Line::styled(
                format!("    {}", sanitize_terminal_text(line)),
                Style::default().fg(theme.foreground),
            ));
        }
    }

    match tool.tool.as_str() {
        "spawn_subagent" => render_spawn_subagent_detail_lines(tool, theme, &mut lines),
        "join_subagent" => render_join_subagent_detail_lines(tool, theme, &mut lines),
        _ => {}
    }

    lines
}

fn render_spawn_subagent_detail_lines(
    tool: &ToolMessageState,
    theme: &Theme,
    lines: &mut Vec<Line<'static>>,
) {
    if let Some(agent_id) = parse_spawn_subagent_agent_id(&tool.detail) {
        lines.push(Line::styled(
            "  Spawned",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::styled(
            format!("    agent_id: {}", sanitize_terminal_text(&agent_id)),
            Style::default().fg(theme.foreground),
        ));
        return;
    }

    append_fallback_tool_output(tool, theme, lines);
}

fn render_join_subagent_detail_lines(
    tool: &ToolMessageState,
    theme: &Theme,
    lines: &mut Vec<Line<'static>>,
) {
    if let Some(terminal) = parse_join_subagent_terminal(&tool.detail) {
        lines.push(Line::styled(
            "  Terminal",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::styled(
            format!("    status: {}", terminal.status),
            Style::default().fg(theme.foreground),
        ));
        if let Some(completed_at_ms) = terminal.completed_at_ms {
            lines.push(Line::styled(
                format!(
                    "    completed_at: {}",
                    format_completed_at_ms(completed_at_ms)
                ),
                Style::default().fg(theme.foreground),
            ));
        }
        if let Some(reply) = terminal.reply {
            lines.push(Line::styled(
                "  Reply",
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            ));
            for line in reply.lines() {
                lines.push(Line::styled(
                    format!("    {}", sanitize_terminal_text(line)),
                    Style::default().fg(theme.foreground),
                ));
            }
        }
        if let Some(error) = terminal.error {
            lines.push(Line::styled(
                "  Error",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ));
            for line in error.lines() {
                lines.push(Line::styled(
                    format!("    {}", sanitize_terminal_text(line)),
                    Style::default().fg(theme.error),
                ));
            }
        }
        return;
    }

    append_fallback_tool_output(tool, theme, lines);
}

fn append_fallback_tool_output(
    tool: &ToolMessageState,
    theme: &Theme,
    lines: &mut Vec<Line<'static>>,
) {
    let detail_text = render_tool_detail_text(&tool.detail);
    let detail_text = detail_text.trim();
    if detail_text.is_empty() {
        lines.push(Line::styled(
            "  No subagent detail available yet.",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        ));
        return;
    }

    lines.push(Line::styled(
        "  Output",
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD),
    ));
    for line in detail_text.lines() {
        lines.push(Line::styled(
            format!("    {}", sanitize_terminal_text(line)),
            Style::default().fg(theme.foreground),
        ));
    }
}

fn parse_spawn_subagent_agent_id(detail: &str) -> Option<String> {
    let value: Value = serde_json::from_str(detail.trim()).ok()?;
    value.get("agent_id")?.as_str().map(ToOwned::to_owned)
}

fn parse_join_subagent_terminal(detail: &str) -> Option<JoinSubagentTerminalDetail> {
    let value: Value = serde_json::from_str(detail.trim()).ok()?;
    let terminal = value.get("terminal")?;
    Some(JoinSubagentTerminalDetail {
        status: terminal.get("status")?.as_str()?.to_string(),
        reply: terminal
            .get("reply")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .filter(|value| !value.trim().is_empty()),
        error: terminal
            .get("error")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .filter(|value| !value.trim().is_empty()),
        completed_at_ms: terminal
            .get("completed_at_ms")
            .and_then(|value| value.as_u64()),
    })
}

fn format_completed_at_ms(value: u64) -> String {
    i64::try_from(value)
        .ok()
        .and_then(|millis| chrono::Local.timestamp_millis_opt(millis).single())
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;
    use ratatui::text::Line;

    use super::{
        highlight_line_selection, parse_join_subagent_terminal, parse_spawn_subagent_agent_id,
        wrap_line_to_visual_lines,
    };

    #[test]
    fn spawn_subagent_detail_parses_agent_id() {
        assert_eq!(
            parse_spawn_subagent_agent_id(r#"{"agent_id":"child-123"}"#),
            Some("child-123".to_string())
        );
    }

    #[test]
    fn join_subagent_detail_parses_terminal_snapshot() {
        let parsed = parse_join_subagent_terminal(
            r#"{"terminal":{"status":"completed","reply":"done","error":null,"completed_at_ms":123}}"#,
        )
        .expect("join_subagent detail should parse");

        assert_eq!(parsed.status, "completed");
        assert_eq!(parsed.reply.as_deref(), Some("done"));
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.completed_at_ms, Some(123));
    }

    #[test]
    fn selection_highlight_preserves_wrapped_visual_layout() {
        let line = Line::from("  assistant output with enough text to wrap");
        let wrapped_before = wrap_line_to_visual_lines(&line.clone(), 12);
        let highlighted = highlight_line_selection(line, 4, 18, Style::default());
        let wrapped_after = wrap_line_to_visual_lines(&highlighted, 12);

        let before_text: Vec<String> = wrapped_before
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        let after_text: Vec<String> = wrapped_after
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();

        assert_eq!(before_text, after_text);
    }
}
