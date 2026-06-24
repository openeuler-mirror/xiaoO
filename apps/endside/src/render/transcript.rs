use chrono::TimeZone;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation},
    Frame,
};
use serde_json::Value;
use textwrap::{wrap, Options, WordSeparator, WordSplitter};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::app_state::{
    CachedMessageLayout, CachedMessageRender, ToolToggleRegion, TranscriptRenderCache,
};
use crate::chat::{Message, MessageRole, ToolExecutionStatus, ToolMessageState};
use crate::markdown::{contains_markdown_table, render_markdown};
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

struct StyleRange {
    start: usize,
    end: usize,
    style: Style,
}

fn merge_spans_with_styles(line: &Line<'static>) -> (String, Vec<StyleRange>) {
    let mut full_text = String::new();
    let mut style_ranges = Vec::new();
    let mut char_offset = 0;

    for span in &line.spans {
        let span_text = &span.content;
        let span_len = span_text.chars().count();

        style_ranges.push(StyleRange {
            start: char_offset,
            end: char_offset + span_len,
            style: span.style,
        });

        full_text.push_str(span_text);
        char_offset += span_len;
    }

    (full_text, style_ranges)
}

fn find_style_at_position(style_ranges: &[StyleRange], pos: usize) -> Style {
    for range in style_ranges {
        if pos >= range.start && pos < range.end {
            return range.style;
        }
    }
    Style::default()
}

fn rebuild_lines_with_styles(
    wrapped_lines: Vec<std::borrow::Cow<'_, str>>,
    style_ranges: &[StyleRange],
    original_line: &Line<'static>,
) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let mut global_char_offset = 0;

    for wrapped_text in wrapped_lines {
        let line_text = wrapped_text.into_owned();
        let line_char_count = line_text.chars().count();

        let mut spans = Vec::new();
        let mut current_style: Option<Style> = None;
        let mut segment_text = String::new();
        let mut local_char_idx = 0;

        for ch in line_text.chars() {
            let global_pos = global_char_offset + local_char_idx;
            let style = find_style_at_position(style_ranges, global_pos);

            if current_style != Some(style) {
                if current_style.is_some() && !segment_text.is_empty() {
                    spans.push(Span::styled(segment_text.clone(), current_style.unwrap()));
                    segment_text.clear();
                }

                current_style = Some(style);
            }

            segment_text.push(ch);
            local_char_idx += 1;
        }

        if !segment_text.is_empty() {
            if let Some(style) = current_style {
                spans.push(Span::styled(segment_text, style));
            }
        }

        let rebuilt_line = Line::from(spans);
        result.push(preserve_line_metadata(rebuilt_line, original_line));

        global_char_offset += line_char_count;
    }

    result
}

fn is_special_width_line(line: &Line<'static>) -> bool {
    for span in &line.spans {
        for ch in span.content.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width > 1 || width == 0 {
                return true;
            }
        }
    }
    false
}

fn wrap_line_by_character(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
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

fn wrap_line_to_visual_lines(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;

    if line.spans.is_empty() {
        return vec![preserve_line_metadata(Line::from(String::new()), line)];
    }

    if is_special_width_line(line) {
        return wrap_line_by_character(line, width as u16);
    }

    let (full_text, style_ranges) = merge_spans_with_styles(line);

    let has_chinese = full_text.chars().any(|c| c > '\u{7F}');
    let word_separator = if has_chinese {
        WordSeparator::UnicodeBreakProperties
    } else {
        WordSeparator::AsciiSpace
    };

    let options = Options::new(width)
        .word_splitter(WordSplitter::NoHyphenation)
        .word_separator(word_separator);
    let wrapped_lines = wrap(&full_text, &options);

    rebuild_lines_with_styles(wrapped_lines, &style_ranges, line)
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
    if tool.tool == "file_edit" {
        if let Some(edit) = parse_file_edit_args(&tool.args_preview) {
            return render_file_edit_tool_lines(message, tool, &edit, tool_color, theme, width);
        }
    }

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
        append_tool_output_detail_lines(&mut lines, detail_text, theme, width, theme.foreground);
    }
    lines.push(Line::raw(""));
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileEditDisplay {
    file_path: String,
    old_string: String,
    new_string: String,
    replace_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffSideKind {
    Context,
    Delete,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffSide {
    line_number: usize,
    text: String,
    kind: DiffSideKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SideBySideDiffRow {
    left: Option<DiffSide>,
    right: Option<DiffSide>,
}

fn parse_file_edit_args(args_preview: &str) -> Option<FileEditDisplay> {
    let value: Value = serde_json::from_str(args_preview.trim()).ok()?;
    Some(FileEditDisplay {
        file_path: value.get("file_path")?.as_str()?.to_string(),
        old_string: value
            .get("old_string")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        new_string: value
            .get("new_string")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        replace_all: value
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn render_file_edit_tool_lines(
    message: &Message,
    tool: &ToolMessageState,
    edit: &FileEditDisplay,
    tool_color: Color,
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
    let diff_rows = build_side_by_side_diff_rows(&edit.old_string, &edit.new_string);
    let (additions, deletions) = diff_change_counts(&diff_rows);
    let replace_all = if edit.replace_all { "  all" } else { "" };
    let mut header = format!(
        "{toggle} Edit {}  {status}  +{additions} -{deletions}{replace_all}",
        edit.file_path
    );
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
                "Edit",
                Style::default().fg(tool_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {timestamp}"), Style::default().fg(theme.muted)),
        ]),
        Line::styled(header, Style::default().fg(tool_color)),
    ];

    if diff_rows.is_empty() {
        lines.push(Line::styled(
            "  No textual change detected.",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        ));
    } else {
        let max_rows = if tool.expanded { 160 } else { 8 };
        append_file_edit_diff_lines(&mut lines, &diff_rows, max_rows, theme, width);
    }

    if tool.expanded && tool.status == ToolExecutionStatus::Failed {
        let detail_text = render_tool_detail_text(&tool.detail);
        let detail_text = detail_text.trim();
        if !detail_text.is_empty() {
            lines.push(Line::styled(
                "  Error",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ));
            append_tool_output_detail_lines(&mut lines, detail_text, theme, width, theme.error);
        }
    }

    lines.push(Line::raw(""));
    lines
}

fn append_file_edit_diff_lines(
    lines: &mut Vec<Line<'static>>,
    diff_rows: &[SideBySideDiffRow],
    max_rows: usize,
    theme: &Theme,
    width: u16,
) {
    let visible_rows = select_diff_rows(diff_rows, max_rows);
    let hidden_rows = diff_rows.len().saturating_sub(visible_rows.len());

    if width >= 56 {
        let header = side_by_side_header(diff_rows, theme, width);
        lines.push(header);
        for row in visible_rows {
            lines.push(render_side_by_side_diff_row(row, diff_rows, theme, width));
        }
    } else {
        for row in visible_rows {
            append_narrow_diff_row(lines, row, theme);
        }
    }

    if hidden_rows > 0 {
        lines.push(Line::styled(
            format!(
                "    {} {hidden_rows} more diff rows",
                sanitize_terminal_text("…")
            ),
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        ));
    }
}

fn side_by_side_header(
    diff_rows: &[SideBySideDiffRow],
    theme: &Theme,
    width: u16,
) -> Line<'static> {
    let (line_number_width, side_width, content_width) = diff_layout(diff_rows, width);
    let gutter = line_number_width + 3;
    let label_width = side_width.saturating_sub(gutter);
    let old_label = pad_to_display_width(
        &truncate_display_width("Original", label_width),
        label_width,
    );
    let new_label =
        pad_to_display_width(&truncate_display_width("Updated", label_width), label_width);

    Line::from(vec![
        Span::raw("    "),
        Span::styled(" ".repeat(gutter), Style::default().fg(theme.muted)),
        Span::styled(old_label, Style::default().fg(theme.muted)),
        Span::styled(" │ ", Style::default().fg(theme.border)),
        Span::styled(" ".repeat(gutter), Style::default().fg(theme.muted)),
        Span::styled(
            pad_to_display_width(&new_label, content_width),
            Style::default().fg(theme.muted),
        ),
    ])
}

fn render_side_by_side_diff_row(
    row: &SideBySideDiffRow,
    all_rows: &[SideBySideDiffRow],
    theme: &Theme,
    width: u16,
) -> Line<'static> {
    let (line_number_width, _side_width, content_width) = diff_layout(all_rows, width);
    let mut spans = vec![Span::raw("    ")];
    spans.extend(render_diff_side(
        row.left.as_ref(),
        line_number_width,
        content_width,
        theme,
    ));
    spans.push(Span::styled(" │ ", Style::default().fg(theme.border)));
    spans.extend(render_diff_side(
        row.right.as_ref(),
        line_number_width,
        content_width,
        theme,
    ));
    Line::from(spans)
}

fn render_diff_side(
    side: Option<&DiffSide>,
    line_number_width: usize,
    content_width: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    match side {
        Some(side) => {
            let marker = match side.kind {
                DiffSideKind::Context => " ",
                DiffSideKind::Delete => "-",
                DiffSideKind::Insert => "+",
            };
            let style = diff_side_style(side.kind, theme);
            let gutter_style = Style::default().fg(match side.kind {
                DiffSideKind::Context => theme.muted,
                DiffSideKind::Delete => theme.error,
                DiffSideKind::Insert => theme.success,
            });
            let content =
                truncate_display_width(&sanitize_terminal_text(&side.text), content_width);
            let content = pad_to_display_width(&content, content_width);
            vec![
                Span::styled(
                    format!("{:>line_number_width$} {marker} ", side.line_number),
                    gutter_style,
                ),
                Span::styled(content, style),
            ]
        }
        None => vec![
            Span::styled(
                " ".repeat(line_number_width + 3),
                Style::default().fg(theme.muted),
            ),
            Span::raw(" ".repeat(content_width)),
        ],
    }
}

fn append_narrow_diff_row(lines: &mut Vec<Line<'static>>, row: &SideBySideDiffRow, theme: &Theme) {
    if let Some(left) = &row.left {
        let marker = match left.kind {
            DiffSideKind::Context => " ",
            DiffSideKind::Delete => "-",
            DiffSideKind::Insert => "+",
        };
        lines.push(Line::styled(
            format!(
                "    {:>4} {marker} {}",
                left.line_number,
                sanitize_terminal_text(&left.text)
            ),
            diff_side_style(left.kind, theme),
        ));
    }
    if let Some(right) = &row.right {
        if row.left.as_ref().is_some_and(|left| {
            left.kind == DiffSideKind::Context && right.kind == DiffSideKind::Context
        }) {
            return;
        }
        let marker = match right.kind {
            DiffSideKind::Context => " ",
            DiffSideKind::Delete => "-",
            DiffSideKind::Insert => "+",
        };
        lines.push(Line::styled(
            format!(
                "    {:>4} {marker} {}",
                right.line_number,
                sanitize_terminal_text(&right.text)
            ),
            diff_side_style(right.kind, theme),
        ));
    }
}

fn diff_layout(diff_rows: &[SideBySideDiffRow], width: u16) -> (usize, usize, usize) {
    let max_line = diff_rows
        .iter()
        .flat_map(|row| {
            [
                row.left.as_ref().map(|side| side.line_number),
                row.right.as_ref().map(|side| side.line_number),
            ]
        })
        .flatten()
        .max()
        .unwrap_or(1);
    let line_number_width = max_line.to_string().len().max(2);
    let available = (width as usize).saturating_sub(7).max(20);
    let side_width = available.saturating_sub(3) / 2;
    let content_width = side_width.saturating_sub(line_number_width + 3).max(4);
    (line_number_width, side_width, content_width)
}

fn diff_side_style(kind: DiffSideKind, theme: &Theme) -> Style {
    match kind {
        DiffSideKind::Context => Style::default().fg(theme.muted),
        DiffSideKind::Delete => Style::default().fg(theme.error).bg(diff_delete_bg(theme)),
        DiffSideKind::Insert => Style::default().fg(theme.success).bg(diff_insert_bg(theme)),
    }
}

fn diff_delete_bg(theme: &Theme) -> Color {
    if theme.is_light() {
        Color::Rgb(255, 229, 229)
    } else {
        Color::Rgb(70, 28, 34)
    }
}

fn diff_insert_bg(theme: &Theme) -> Color {
    if theme.is_light() {
        Color::Rgb(224, 245, 226)
    } else {
        Color::Rgb(24, 60, 36)
    }
}

fn select_diff_rows(diff_rows: &[SideBySideDiffRow], max_rows: usize) -> Vec<&SideBySideDiffRow> {
    if diff_rows.len() <= max_rows {
        return diff_rows.iter().collect();
    }

    let mut selected = Vec::new();
    let mut included = vec![false; diff_rows.len()];
    for (index, row) in diff_rows.iter().enumerate() {
        if !row_has_change(row) {
            continue;
        }
        for include_index in index.saturating_sub(1)..=(index + 1).min(diff_rows.len() - 1) {
            if included[include_index] || selected.len() >= max_rows {
                continue;
            }
            included[include_index] = true;
            selected.push(&diff_rows[include_index]);
        }
        if selected.len() >= max_rows {
            break;
        }
    }

    if selected.is_empty() {
        diff_rows.iter().take(max_rows).collect()
    } else {
        selected
    }
}

fn row_has_change(row: &SideBySideDiffRow) -> bool {
    row.left
        .as_ref()
        .is_some_and(|side| side.kind != DiffSideKind::Context)
        || row
            .right
            .as_ref()
            .is_some_and(|side| side.kind != DiffSideKind::Context)
}

fn diff_change_counts(rows: &[SideBySideDiffRow]) -> (usize, usize) {
    let additions = rows
        .iter()
        .filter(|row| {
            row.right
                .as_ref()
                .is_some_and(|side| side.kind == DiffSideKind::Insert)
        })
        .count();
    let deletions = rows
        .iter()
        .filter(|row| {
            row.left
                .as_ref()
                .is_some_and(|side| side.kind == DiffSideKind::Delete)
        })
        .count();
    (additions, deletions)
}

fn build_side_by_side_diff_rows(old_text: &str, new_text: &str) -> Vec<SideBySideDiffRow> {
    let old_lines = display_lines(old_text);
    let new_lines = display_lines(new_text);
    let edits = line_diff_edits(&old_lines, &new_lines);
    let mut rows = Vec::new();
    let mut old_line_number = 1usize;
    let mut new_line_number = 1usize;
    let mut index = 0usize;

    while index < edits.len() {
        match &edits[index] {
            LineDiffEdit::Equal(text) => {
                rows.push(SideBySideDiffRow {
                    left: Some(DiffSide {
                        line_number: old_line_number,
                        text: text.clone(),
                        kind: DiffSideKind::Context,
                    }),
                    right: Some(DiffSide {
                        line_number: new_line_number,
                        text: text.clone(),
                        kind: DiffSideKind::Context,
                    }),
                });
                old_line_number += 1;
                new_line_number += 1;
                index += 1;
            }
            LineDiffEdit::Delete(_) | LineDiffEdit::Insert(_) => {
                let mut deletes = Vec::new();
                let mut inserts = Vec::new();
                while index < edits.len() {
                    match &edits[index] {
                        LineDiffEdit::Delete(text) => {
                            deletes.push((old_line_number, text.clone()));
                            old_line_number += 1;
                        }
                        LineDiffEdit::Insert(text) => {
                            inserts.push((new_line_number, text.clone()));
                            new_line_number += 1;
                        }
                        LineDiffEdit::Equal(_) => break,
                    }
                    index += 1;
                }
                let row_count = deletes.len().max(inserts.len());
                for row_index in 0..row_count {
                    rows.push(SideBySideDiffRow {
                        left: deletes.get(row_index).map(|(line_number, text)| DiffSide {
                            line_number: *line_number,
                            text: text.clone(),
                            kind: DiffSideKind::Delete,
                        }),
                        right: inserts.get(row_index).map(|(line_number, text)| DiffSide {
                            line_number: *line_number,
                            text: text.clone(),
                            kind: DiffSideKind::Insert,
                        }),
                    });
                }
            }
        }
    }

    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LineDiffEdit {
    Equal(String),
    Delete(String),
    Insert(String),
}

fn line_diff_edits(old_lines: &[String], new_lines: &[String]) -> Vec<LineDiffEdit> {
    if old_lines.is_empty() {
        return new_lines
            .iter()
            .cloned()
            .map(LineDiffEdit::Insert)
            .collect();
    }
    if new_lines.is_empty() {
        return old_lines
            .iter()
            .cloned()
            .map(LineDiffEdit::Delete)
            .collect();
    }

    let cell_count = old_lines.len().saturating_mul(new_lines.len());
    if cell_count > 20_000 {
        return large_diff_edits(old_lines, new_lines);
    }

    let old_len = old_lines.len();
    let new_len = new_lines.len();
    let mut lcs = vec![vec![0usize; new_len + 1]; old_len + 1];
    for old_index in (0..old_len).rev() {
        for new_index in (0..new_len).rev() {
            lcs[old_index][new_index] = if old_lines[old_index] == new_lines[new_index] {
                lcs[old_index + 1][new_index + 1] + 1
            } else {
                lcs[old_index + 1][new_index].max(lcs[old_index][new_index + 1])
            };
        }
    }

    let mut edits = Vec::new();
    let mut old_index = 0usize;
    let mut new_index = 0usize;
    while old_index < old_len && new_index < new_len {
        if old_lines[old_index] == new_lines[new_index] {
            edits.push(LineDiffEdit::Equal(old_lines[old_index].clone()));
            old_index += 1;
            new_index += 1;
        } else if lcs[old_index + 1][new_index] >= lcs[old_index][new_index + 1] {
            edits.push(LineDiffEdit::Delete(old_lines[old_index].clone()));
            old_index += 1;
        } else {
            edits.push(LineDiffEdit::Insert(new_lines[new_index].clone()));
            new_index += 1;
        }
    }
    edits.extend(
        old_lines[old_index..]
            .iter()
            .cloned()
            .map(LineDiffEdit::Delete),
    );
    edits.extend(
        new_lines[new_index..]
            .iter()
            .cloned()
            .map(LineDiffEdit::Insert),
    );
    edits
}

fn large_diff_edits(old_lines: &[String], new_lines: &[String]) -> Vec<LineDiffEdit> {
    let mut prefix_len = 0usize;
    while prefix_len < old_lines.len().min(new_lines.len())
        && old_lines[prefix_len] == new_lines[prefix_len]
    {
        prefix_len += 1;
    }

    let mut suffix_len = 0usize;
    while suffix_len < old_lines.len().saturating_sub(prefix_len)
        && suffix_len < new_lines.len().saturating_sub(prefix_len)
        && old_lines[old_lines.len() - 1 - suffix_len]
            == new_lines[new_lines.len() - 1 - suffix_len]
    {
        suffix_len += 1;
    }

    let mut edits = Vec::new();
    edits.extend(
        old_lines[..prefix_len]
            .iter()
            .cloned()
            .map(LineDiffEdit::Equal),
    );
    edits.extend(
        old_lines[prefix_len..old_lines.len() - suffix_len]
            .iter()
            .cloned()
            .map(LineDiffEdit::Delete),
    );
    edits.extend(
        new_lines[prefix_len..new_lines.len() - suffix_len]
            .iter()
            .cloned()
            .map(LineDiffEdit::Insert),
    );
    edits.extend(
        old_lines[old_lines.len() - suffix_len..]
            .iter()
            .cloned()
            .map(LineDiffEdit::Equal),
    );
    edits
}

fn display_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.lines().map(ToOwned::to_owned).collect()
    }
}

fn pad_to_display_width(text: &str, width: usize) -> String {
    let used_width: usize = text
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum();
    if used_width >= width {
        text.to_string()
    } else {
        format!("{}{}", text, " ".repeat(width - used_width))
    }
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
        "spawn_subagent" => render_spawn_subagent_detail_lines(tool, theme, width, &mut lines),
        "join_subagent" => render_join_subagent_detail_lines(tool, theme, width, &mut lines),
        _ => {}
    }

    lines
}

fn render_spawn_subagent_detail_lines(
    tool: &ToolMessageState,
    theme: &Theme,
    width: u16,
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

    append_fallback_tool_output(tool, theme, width, lines);
}

fn render_join_subagent_detail_lines(
    tool: &ToolMessageState,
    theme: &Theme,
    width: u16,
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
            append_tool_output_detail_lines(lines, &reply, theme, width, theme.foreground);
        }
        if let Some(error) = terminal.error {
            lines.push(Line::styled(
                "  Error",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ));
            append_tool_output_detail_lines(lines, &error, theme, width, theme.error);
        }
        return;
    }

    append_fallback_tool_output(tool, theme, width, lines);
}

fn append_fallback_tool_output(
    tool: &ToolMessageState,
    theme: &Theme,
    width: u16,
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
    append_tool_output_detail_lines(lines, detail_text, theme, width, theme.foreground);
}

fn append_tool_output_detail_lines(
    lines: &mut Vec<Line<'static>>,
    detail_text: &str,
    theme: &Theme,
    width: u16,
    fallback_color: Color,
) {
    const OUTPUT_INDENT: &str = "    ";

    if contains_markdown_table(detail_text) {
        let content_width = width.saturating_sub(OUTPUT_INDENT.len() as u16).max(1);
        for line in render_markdown(detail_text, theme, content_width) {
            lines.push(prefix_line(
                line,
                OUTPUT_INDENT,
                Style::default().fg(theme.muted),
            ));
        }
        return;
    }

    for line in detail_text.lines() {
        lines.push(Line::styled(
            format!("{}{}", OUTPUT_INDENT, sanitize_terminal_text(line)),
            Style::default().fg(fallback_color),
        ));
    }
}

fn prefix_line(line: Line<'static>, prefix: &str, prefix_style: Style) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(prefix.to_string(), prefix_style));
    spans.extend(line.spans);
    let mut prefixed = Line::from(spans);
    prefixed.style = line.style;
    prefixed.alignment = line.alignment;
    prefixed
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
    use ratatui::style::{Color, Style};
    use ratatui::text::Line;

    use crate::chat::{Message, ToolExecutionStatus, ToolExecutionUpdate};
    use crate::theme::Theme;

    use super::{
        build_side_by_side_diff_rows, diff_change_counts, highlight_line_selection,
        parse_file_edit_args, parse_join_subagent_terminal, parse_spawn_subagent_agent_id,
        render_file_edit_tool_lines, render_tool_message_lines, wrap_line_to_visual_lines,
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

    #[test]
    fn file_edit_args_parse_display_fields() {
        let args = serde_json::json!({
            "file_path": "README.md",
            "old_string": "before\n",
            "new_string": "after\n",
            "replace_all": true
        })
        .to_string();

        let parsed = parse_file_edit_args(&args).expect("file_edit args should parse");

        assert_eq!(parsed.file_path, "README.md");
        assert_eq!(parsed.old_string, "before\n");
        assert_eq!(parsed.new_string, "after\n");
        assert!(parsed.replace_all);
    }

    #[test]
    fn tool_output_renders_markdown_tables_when_expanded() {
        let theme = Theme::detect();
        let message = Message::tool_event(ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "bash".to_string(),
            summary: String::new(),
            args_preview: String::new(),
            command_preview: None,
            command: None,
            detail: "| Name | Status |\n| --- | --- |\n| xiaoO | ready |".to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: Some(0),
            duration_ms: Some(10),
            file_change: None,
        });
        let mut tool = message
            .tool_state
            .clone()
            .expect("tool message should carry tool state");
        tool.expanded = true;

        let lines = render_tool_message_lines(&message, &tool, Color::Green, &theme, 80);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(text.iter().any(|line| line.contains("┌")));
        assert!(text.iter().any(|line| line.contains("xiaoO")));
        assert!(!text.iter().any(|line| line.contains("| --- | --- |")));
    }

    #[test]
    fn side_by_side_diff_pairs_replacement_lines() {
        let rows = build_side_by_side_diff_rows("one\ntwo\nthree\n", "one\ndeux\nthree\n");
        let (additions, deletions) = diff_change_counts(&rows);

        assert_eq!((additions, deletions), (1, 1));
        let changed = rows
            .iter()
            .find(|row| row.left.as_ref().is_some_and(|side| side.text == "two"))
            .expect("replacement row should exist");
        assert_eq!(
            changed.right.as_ref().map(|side| side.text.as_str()),
            Some("deux")
        );
    }

    #[test]
    fn file_edit_render_includes_path_and_stats() {
        let args = serde_json::json!({
            "file_path": "README.md",
            "old_string": "before\n",
            "new_string": "after\n"
        })
        .to_string();
        let edit = parse_file_edit_args(&args).expect("file_edit args should parse");
        let message = Message::tool_event(ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "file_edit".to_string(),
            summary: String::new(),
            args_preview: args,
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        });
        let tool = message
            .tool_state
            .as_ref()
            .expect("tool message should carry tool state");

        let lines =
            render_file_edit_tool_lines(&message, tool, &edit, Color::Green, &Theme::detect(), 80);
        let rendered_text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered_text.contains("Edit README.md"));
        assert!(rendered_text.contains("+1 -1"));
        assert!(rendered_text.contains("Original"));
        assert!(rendered_text.contains("Updated"));
    }
}
