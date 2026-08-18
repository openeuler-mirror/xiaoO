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
    CachedMessageRender, MessageVisualBlock, SubagentOpenRegion, SubagentOpenTarget,
    ToolToggleRegion, TranscriptRenderCache,
};
use crate::chat::{Message, MessageRole, ToolExecutionStatus, ToolMessageState};
use crate::markdown::{
    contains_markdown_table, render_markdown, render_markdown_incremental, MarkdownIncrementalState,
};
use crate::theme::Theme;

use super::utils::{
    find_substring_from, render_tool_detail_text, sanitize_terminal_text, truncate_display_width,
};

impl App {
    pub(crate) fn render_chat(&mut self, frame: &mut Frame, area: Rect) {
        #[cfg(debug_assertions)]
        let _start = std::time::Instant::now();
        #[cfg(debug_assertions)]
        let mut _t_dirty: Option<std::time::Instant> = None;
        #[cfg(debug_assertions)]
        let mut _t_build: Option<std::time::Instant> = None;
        #[cfg(debug_assertions)]
        let mut _t_collect: Option<std::time::Instant> = None;
        let transcript_key = self.state.active_transcript_key();
        let active_agent_id = self
            .state
            .chat_state
            .active_subagent_id()
            .filter(|agent_id| self.state.chat_state.subagent_lanes.contains_key(*agent_id))
            .map(ToOwned::to_owned);
        let title = self
            .state
            .active_subagent_title()
            .map(|title| format!(" {title} | ← Back "))
            .unwrap_or_else(|| " Messages ".to_string());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .title(sanitize_terminal_text(&title))
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
        let root_stream_index = self.gateway.stream_message_index;
        let theme = self.state.theme;

        {
            let chat_state = &self.state.chat_state;
            let render_state = &mut self.state.render_state;
            if render_state.active_transcript_key.as_deref() != Some(transcript_key.as_str()) {
                // Switched transcript (different session / subagent lane):
                // full rebuild — prior blocks belong to a different message
                // stream and must not be reused.
                render_state.message_render_revisions.clear();
                render_state.last_render_width = None;
                render_state.last_render_theme = None;
                render_state.transcript_cache = None;
                render_state.incremental_markdown = None;
                render_state.incremental_markdown_index = None;
                render_state.active_transcript_key = Some(transcript_key.clone());
            }

            let (messages, active_stream_index, chat_is_loading) =
                if let Some(agent_id) = active_agent_id.as_deref() {
                    let lane = chat_state
                        .subagent_lanes
                        .get(agent_id)
                        .expect("active subagent lane should exist");
                    (&lane.messages, lane.stream_message_index, lane.is_running)
                } else {
                    (
                        &chat_state.messages,
                        root_stream_index,
                        chat_state.is_loading,
                    )
                };

            let message_count = messages.len();
            let prev_count = render_state.message_render_revisions.len();
            if prev_count > message_count {
                // Messages removed from the middle/end: surviving indices may
                // have shifted, so a per-index `render_revision` check could
                // match a different message's block. Fall back to a full
                // rebuild (rare path — deletions are uncommon).
                render_state.message_render_revisions = vec![None; message_count];
                render_state.last_render_width = None;
                render_state.last_render_theme = None;
                render_state.transcript_cache = None;
                render_state.incremental_markdown = None;
                render_state.incremental_markdown_index = None;
            } else if prev_count < message_count {
                // Messages appended: extend revision slots with `None` so the
                // new tail is dirty. Prior blocks stay valid (append preserves
                // indices), so `transcript_cache` is NOT cleared — the new
                // messages are added incrementally on top.
                render_state
                    .message_render_revisions
                    .resize(message_count, None);
            }

            let width_changed = render_state.last_render_width != Some(inner_area.width);
            let theme_changed = render_state.last_render_theme != Some(theme);

            // The incremental markdown cache belongs only to the active
            // streaming message. If the stream moved to a different index
            // (settled / switched / none), clear it so a stale cache is
            // never reused.
            if render_state.incremental_markdown_index != active_stream_index {
                render_state.incremental_markdown = None;
                render_state.incremental_markdown_index = active_stream_index;
            }

            // A None `transcript_cache` (first tick, post-switch, post-shrink)
            // forces every message dirty; width/theme changes likewise.
            let mut renders: Vec<Option<CachedMessageRender>> = Vec::with_capacity(message_count);
            let mut any_dirty =
                render_state.transcript_cache.is_none() || width_changed || theme_changed;

            for message_index in 0..message_count {
                let message = &messages[message_index];
                let is_active_stream_message = active_stream_index == Some(message_index);
                let should_bypass_cache = is_active_stream_message && chat_is_loading;
                let revision_changed = render_state.message_render_revisions[message_index]
                    != Some(message.render_revision);
                let is_dirty =
                    should_bypass_cache || revision_changed || width_changed || theme_changed;

                if is_dirty {
                    // Only the active streaming message uses the incremental
                    // markdown cache; every other dirty message renders fresh.
                    let prev_markdown_state = if is_active_stream_message {
                        render_state.incremental_markdown.take()
                    } else {
                        None
                    };
                    let (rendered, new_markdown_state) = render_message_entry(
                        message,
                        &theme,
                        inner_area.width,
                        is_active_stream_message,
                        chat_is_loading,
                        &loading_animation,
                        prev_markdown_state,
                    );
                    // Persist the fingerprint only for non-bypass messages:
                    // bypass (active stream) messages are recomputed every
                    // tick by definition, so leaving their slot untouched
                    // keeps them dirty until the stream settles — at which
                    // point `revision_changed` fires once and the slot is
                    // updated.
                    if !should_bypass_cache {
                        render_state.message_render_revisions[message_index] =
                            Some(message.render_revision);
                    }
                    if is_active_stream_message {
                        render_state.incremental_markdown = new_markdown_state;
                    }
                    renders.push(Some(rendered));
                    any_dirty = true;
                } else {
                    // Non-dirty: `build_transcript_cache` will move the prior
                    // block for this message out of the prev cache (zero
                    // `Line` clone).
                    renders.push(None);
                }
            }

            render_state.last_render_width = Some(inner_area.width);
            render_state.last_render_theme = Some(theme);

            #[cfg(debug_assertions)]
            {
                _t_dirty = Some(std::time::Instant::now());
            }
            if any_dirty {
                let prev = render_state.transcript_cache.take();
                render_state.transcript_cache = Some(build_transcript_cache(prev, renders));
            }
            #[cfg(debug_assertions)]
            {
                _t_build = Some(std::time::Instant::now());
            }
            // `any_dirty == false` && prev cache present: nothing changed this
            // tick — reuse the cache untouched (pure scroll / cursor blink).
        }

        let transcript_cache = self
            .state
            .render_state
            .transcript_cache
            .as_ref()
            .expect("transcript cache must be populated");

        let max_scroll = transcript_cache
            .total_lines
            .saturating_sub(inner_height)
            .min(transcript_cache.total_lines);
        let scroll_offset = if let Some(agent_id) = active_agent_id.as_deref() {
            let lane = self
                .state
                .chat_state
                .subagent_lanes
                .get_mut(agent_id)
                .expect("active subagent lane should exist");
            lane.total_lines = transcript_cache.total_lines;
            lane.last_visible_height = inner_height;
            if lane.stick_to_bottom {
                lane.scroll_offset = max_scroll;
            } else {
                lane.scroll_offset = lane.scroll_offset.min(max_scroll);
            }
            lane.scroll_offset
        } else {
            self.state.chat_state.total_lines = transcript_cache.total_lines;
            self.state.chat_state.last_visible_height = inner_height;
            if self.state.chat_state.stick_to_bottom {
                self.state.chat_state.scroll_offset = max_scroll;
            } else {
                self.state.chat_state.scroll_offset =
                    self.state.chat_state.scroll_offset.min(max_scroll);
            }
            self.state.chat_state.scroll_offset
        };
        let scroll_end = scroll_offset.saturating_add(inner_height);
        paint_visible_line_backgrounds(frame, inner_area, transcript_cache, scroll_offset);
        if let Some(sel) = &self.state.transcript_selection {
            let logical_line_count = transcript_cache.logical_line_count();
            let start_line_index = transcript_cache
                .logical_line_visual_starts
                .partition_point(|start| *start <= scroll_offset)
                .saturating_sub(1);
            let safe_start_line_index = start_line_index.min(logical_line_count.saturating_sub(1));
            let slice_start_visual = transcript_cache
                .logical_line_visual_starts
                .get(safe_start_line_index)
                .copied()
                .unwrap_or(0);
            let paragraph_scroll = scroll_offset.saturating_sub(slice_start_visual);

            let mut end_line_index = safe_start_line_index;
            while end_line_index < logical_line_count {
                let line_start = transcript_cache.logical_line_visual_starts[end_line_index];
                if line_start >= scroll_end {
                    break;
                }
                end_line_index += 1;
            }
            if end_line_index == safe_start_line_index && end_line_index < logical_line_count {
                end_line_index += 1;
            }

            let (start_line, start_col, end_line, end_col) = sel.normalised();
            let sel_style = Style::default()
                .fg(self.state.theme.background)
                .bg(self.state.theme.foreground)
                .add_modifier(Modifier::BOLD);
            let mut selected_visual_lines = Vec::new();
            for visible_index in safe_start_line_index..end_line_index {
                let global_line_index = visible_index;
                let Some(original_line) = transcript_cache.logical_line(global_line_index) else {
                    continue;
                };
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
            let visual_end = scroll_end.min(transcript_cache.total_lines);
            #[cfg(debug_assertions)]
            {
                _t_collect = Some(std::time::Instant::now());
            }
            let visible_visual_lines =
                transcript_cache.collect_visible_visual_lines(scroll_offset, visual_end);
            let paragraph = Paragraph::new(Text::from(visible_visual_lines));
            frame.render_widget(paragraph, inner_area);
        }

        self.state.render_state.tool_toggle_regions.clear();
        self.state.render_state.subagent_open_regions.clear();
        for block in &transcript_cache.message_blocks {
            if let Some(open_target) = &block.subagent_open_target {
                let open_row = block
                    .start_visual_row
                    .saturating_add(open_target.row_offset);
                if open_row >= scroll_offset && open_row < scroll_end {
                    self.state
                        .render_state
                        .subagent_open_regions
                        .push(SubagentOpenRegion {
                            agent_id: open_target.agent_id.clone(),
                            rect: Rect {
                                x: inner_area.x,
                                y: inner_area.y + (open_row.saturating_sub(scroll_offset) as u16),
                                width: inner_area.width,
                                height: 1,
                            },
                        });
                }
            }

            if let Some(toggle_row_offset) = block.tool_toggle_row_offset {
                let toggle_row = block.start_visual_row.saturating_add(toggle_row_offset);
                if toggle_row >= scroll_offset && toggle_row < scroll_end {
                    self.state
                        .render_state
                        .tool_toggle_regions
                        .push(ToolToggleRegion {
                            message_index: block.message_index,
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

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .style(Style::default().fg(self.state.theme.border));
        if let Some(agent_id) = active_agent_id.as_deref() {
            let lane = self
                .state
                .chat_state
                .subagent_lanes
                .get_mut(agent_id)
                .expect("active subagent lane should exist");
            lane.sync_scrollbar_state();
            frame.render_stateful_widget(scrollbar, scrollbar_area, &mut lane.scrollbar_state);
        } else {
            self.state.chat_state.sync_scrollbar_state();
            frame.render_stateful_widget(
                scrollbar,
                scrollbar_area,
                &mut self.state.chat_state.scrollbar_state,
            );
        }

        #[cfg(debug_assertions)]
        {
            let elapsed = _start.elapsed();
            // Phase breakdown (debug only). Phases:
            //  - dirty:   dirty-detection loop + render_message_entry for dirty msgs
            //  - build:   build_transcript_cache (incremental or full)
            //  - collect: collect_visible_visual_lines (the sole per-frame Line clone)
            //  - render:  Paragraph::render_widget + scrollbar + region scan
            // `dirty`+`build` only accumulate when any_dirty; `collect`+`render`
            // happen every frame. Phases are sequential and non-overlapping,
            // so they sum to ~`elapsed`.
            if elapsed > std::time::Duration::from_micros(500) {
                let t_dirty = _t_dirty
                    .map(|t| t.duration_since(_start))
                    .unwrap_or_default();
                let t_build = match (_t_dirty, _t_build) {
                    (Some(d), Some(b)) => b.duration_since(d),
                    _ => std::time::Duration::ZERO,
                };
                let t_collect = match (_t_build, _t_collect) {
                    (Some(b), Some(c)) => c.duration_since(b),
                    (None, Some(c)) => c.duration_since(_start),
                    _ => std::time::Duration::ZERO,
                };
                let t_render = match _t_collect {
                    Some(c) => _start.elapsed().saturating_sub(c.duration_since(_start)),
                    None => std::time::Duration::ZERO,
                };
                // tracing, NEVER eprintln!/println!: raw stdout/stderr writes
                // land at the cursor position inside the live TUI screen.
                tracing::debug!(
                    "PERF render_chat: {}µs dirty={}µs build={}µs collect={}µs render={}µs",
                    elapsed.as_micros(),
                    t_dirty.as_micros(),
                    t_build.as_micros(),
                    t_collect.as_micros(),
                    t_render.as_micros(),
                );
            }
        }
    }
}

fn render_message_entry(
    message: &Message,
    theme: &Theme,
    width: u16,
    is_active_stream_message: bool,
    chat_is_loading: bool,
    loading_animation: &str,
    incremental_markdown: Option<MarkdownIncrementalState>,
) -> (CachedMessageRender, Option<MarkdownIncrementalState>) {
    let mut tool_toggle_row_offset = None;
    let mut subagent_open_target = None;

    if let Some(tool) = &message.tool_state {
        let tool_color = match tool.status {
            ToolExecutionStatus::Running => theme.accent,
            ToolExecutionStatus::Completed => theme.success,
            ToolExecutionStatus::Failed => theme.error,
        };
        let timestamp = message.timestamp.format("%H:%M:%S").to_string();
        let lines = if is_subagent_tool(&tool.tool) {
            tool_toggle_row_offset = Some(if tool.expanded { 1 } else { 0 });
            let mut lines = render_subagent_tool_lines(tool, &timestamp, tool_color, theme, width);
            if tool.tool == "spawn_subagent" && tool.expanded {
                if let Some(agent_id) = parse_spawn_subagent_agent_id(&tool.detail) {
                    if let Some(row_offset) = lines
                        .iter()
                        .position(|line| line_plain_text(line).contains("agent_id:"))
                    {
                        subagent_open_target = Some(SubagentOpenTarget {
                            agent_id,
                            row_offset,
                        });
                    }
                }
            }
            lines.push(Line::raw(""));
            lines
        } else {
            tool_toggle_row_offset = Some(if tool.expanded { 2 } else { 1 });
            render_tool_message_lines(message, tool, tool_color, theme, width)
        };
        let wrapped_lines: Vec<Vec<Line<'static>>> = lines
            .iter()
            .map(|line| wrap_line_to_visual_lines(line, width))
            .collect();
        return (
            CachedMessageRender {
                width,
                tool_toggle_row_offset,
                subagent_open_target,
                wrapped_lines: Some(wrapped_lines),
                lines,
                frozen_prefix_line_count: None,
            },
            None,
        );
    }

    if let Some(checker) = &message.completion_check_state {
        let lines = render_completion_check_lines(message, checker, theme);
        let wrapped_lines: Vec<Vec<Line<'static>>> = lines
            .iter()
            .map(|line| wrap_line_to_visual_lines(line, width))
            .collect();
        return (
            CachedMessageRender {
                width,
                tool_toggle_row_offset,
                subagent_open_target,
                wrapped_lines: Some(wrapped_lines),
                lines,
                frozen_prefix_line_count: None,
            },
            None,
        );
    }

    let (lines, wrapped_lines, new_markdown_state, frozen_prefix_line_count) =
        render_standard_message_lines(
            message,
            theme,
            width,
            is_active_stream_message,
            chat_is_loading,
            loading_animation,
            incremental_markdown,
        );

    (
        CachedMessageRender {
            width,
            tool_toggle_row_offset,
            subagent_open_target,
            wrapped_lines: Some(wrapped_lines),
            lines,
            frozen_prefix_line_count,
        },
        new_markdown_state,
    )
}

pub(crate) fn build_transcript_cache(
    prev: Option<TranscriptRenderCache>,
    renders: Vec<Option<CachedMessageRender>>,
) -> TranscriptRenderCache {
    #[cfg(debug_assertions)]
    let _start = std::time::Instant::now();

    // Take ownership of the previous cache's message blocks so non-dirty
    // messages can be MOVED into the new cache (zero `Line` clone). Each slot
    // is `Option` so we can `take()` exactly one block per non-dirty message.
    let mut prev_blocks: Vec<Option<MessageVisualBlock>> = prev
        .map(|c| {
            let TranscriptRenderCache { message_blocks, .. } = c;
            message_blocks.into_iter().map(Some).collect()
        })
        .unwrap_or_default();

    let mut message_blocks: Vec<MessageVisualBlock> = Vec::with_capacity(renders.len());
    let mut logical_line_visual_starts: Vec<usize> = Vec::new();
    let mut line_texts: Vec<String> = Vec::new();
    let mut line_is_header: Vec<bool> = Vec::new();
    let mut visual_line_backgrounds: Vec<Option<Color>> = Vec::new();
    let mut absolute_visual_row = 0usize;
    let mut absolute_logical_row = 0usize;

    for (message_index, render_opt) in renders.into_iter().enumerate() {
        let block = if let Some(render) = render_opt {
            if let Some(freeze_n) = render.frozen_prefix_line_count {
                // Incremental streaming: move the frozen prefix (`freeze_n`
                // logical lines) from the previous tick's block, then append
                // the freshly rendered suffix. Zero `Line` clone for the
                // frozen prefix — this is what makes per-tick work O(suffix)
                // instead of O(total).
                let prev_b = prev_blocks.get_mut(message_index).and_then(Option::take);
                match prev_b {
                    Some(MessageVisualBlock {
                        mut lines,
                        mut logical_to_visual_offset,
                        mut visual_lines,
                        ..
                    }) => {
                        debug_assert!(
                            lines.len() >= freeze_n,
                            "build_transcript_cache: prev block for message {message_index} \
                             has {} lines but freeze_n={freeze_n}",
                            lines.len()
                        );
                        // Trim the prev block to the frozen prefix (drop the
                        // old suffix — it is re-rendered this tick).
                        let visual_freeze_n = logical_to_visual_offset
                            .get(freeze_n)
                            .copied()
                            .unwrap_or(visual_lines.len());
                        lines.truncate(freeze_n);
                        logical_to_visual_offset.truncate(freeze_n);
                        visual_lines.truncate(visual_freeze_n);

                        // Append the freshly rendered suffix.
                        let suffix_lines = render.lines;
                        let suffix_wrapped = render.wrapped_lines.unwrap_or_else(|| {
                            suffix_lines
                                .iter()
                                .map(|l| wrap_line_to_visual_lines(l, render.width))
                                .collect::<Vec<_>>()
                        });
                        let mut acc = visual_freeze_n;
                        for wl in &suffix_wrapped {
                            logical_to_visual_offset.push(acc);
                            acc += wl.len();
                        }
                        for wl in suffix_wrapped {
                            visual_lines.extend(wl);
                        }
                        lines.extend(suffix_lines);

                        MessageVisualBlock {
                            message_index,
                            start_visual_row: absolute_visual_row,
                            logical_line_start: absolute_logical_row,
                            lines,
                            visual_lines,
                            logical_to_visual_offset,
                            tool_toggle_row_offset: None,
                            subagent_open_target: None,
                        }
                    }
                    None => {
                        // Invariant: Some(freeze_n) implies a prev block
                        // exists (established in render_chat — the active
                        // stream message is always dirty, so it has a block
                        // from the prior tick). If we get here, fall back to a
                        // suffix-only block; the prefix is missing this tick
                        // but corrected on the next.
                        debug_assert!(
                            false,
                            "build_transcript_cache: incremental render for message \
                             {message_index} has frozen_prefix_line_count={freeze_n} \
                             but no prev block"
                        );
                        let suffix_lines = render.lines;
                        let suffix_wrapped = render.wrapped_lines.unwrap_or_else(|| {
                            suffix_lines
                                .iter()
                                .map(|l| wrap_line_to_visual_lines(l, render.width))
                                .collect::<Vec<_>>()
                        });
                        let mut l2v = Vec::with_capacity(suffix_wrapped.len());
                        let mut acc = 0usize;
                        for wl in &suffix_wrapped {
                            l2v.push(acc);
                            acc += wl.len();
                        }
                        let mut vl: Vec<Line<'static>> = Vec::with_capacity(acc);
                        for wl in suffix_wrapped {
                            vl.extend(wl);
                        }
                        MessageVisualBlock {
                            message_index,
                            start_visual_row: absolute_visual_row,
                            logical_line_start: absolute_logical_row,
                            lines: suffix_lines,
                            visual_lines: vl,
                            logical_to_visual_offset: l2v,
                            tool_toggle_row_offset: None,
                            subagent_open_target: None,
                        }
                    }
                }
            } else {
                // Full dirty rebuild: move the freshly-computed render
                // (no `Line` clone of the wrapped trees).
                let lines = render.lines;
                let wrapped_lines = render.wrapped_lines.unwrap_or_else(|| {
                    lines
                        .iter()
                        .map(|line| wrap_line_to_visual_lines(line, render.width))
                        .collect::<Vec<_>>()
                });

                // Per-logical-line visual row offset within this block; needed
                // both to convert `tool_toggle_row_offset` /
                // `subagent_open_target` (logical → visual) and to fill the
                // flat `logical_line_visual_starts` index without re-wrapping.
                let mut logical_to_visual_offset: Vec<usize> =
                    Vec::with_capacity(wrapped_lines.len());
                let mut acc = 0usize;
                for wl in &wrapped_lines {
                    logical_to_visual_offset.push(acc);
                    acc += wl.len();
                }

                let mut visual_lines: Vec<Line<'static>> = Vec::with_capacity(acc);
                for wl in wrapped_lines {
                    visual_lines.extend(wl);
                }

                let tool_toggle_row_offset = render.tool_toggle_row_offset.map(|logical_offset| {
                    logical_to_visual_offset
                        .get(logical_offset)
                        .copied()
                        .unwrap_or(acc)
                });
                let subagent_open_target =
                    render
                        .subagent_open_target
                        .as_ref()
                        .map(|target| SubagentOpenTarget {
                            agent_id: target.agent_id.clone(),
                            row_offset: logical_to_visual_offset
                                .get(target.row_offset)
                                .copied()
                                .unwrap_or(acc),
                        });

                MessageVisualBlock {
                    message_index,
                    start_visual_row: absolute_visual_row,
                    logical_line_start: absolute_logical_row,
                    lines,
                    visual_lines,
                    logical_to_visual_offset,
                    tool_toggle_row_offset,
                    subagent_open_target,
                }
            }
        } else {
            // Non-dirty: move the prior block out of prev_blocks. Its
            // `lines` / `visual_lines` / `logical_to_visual_offset` /
            // `tool_toggle_row_offset` / `subagent_open_target` carry over
            // untouched; only the global offsets need re-basing.
            let mut moved = match prev_blocks.get_mut(message_index).and_then(Option::take) {
                Some(b) => b,
                None => {
                    // Caller invariant: a `None` render requires a matching
                    // prev block. If we get here the message_count shrank
                    // without a full-rebuild fallback (see render_chat),
                    // which is a bug. Fall back to an empty block to avoid a
                    // panic; the assertion in debug builds catches the
                    // upstream mistake.
                    debug_assert!(
                        false,
                        "build_transcript_cache: non-dirty message {message_index} \
                         has no prev block"
                    );
                    MessageVisualBlock {
                        message_index,
                        start_visual_row: absolute_visual_row,
                        logical_line_start: absolute_logical_row,
                        lines: Vec::new(),
                        visual_lines: Vec::new(),
                        logical_to_visual_offset: Vec::new(),
                        tool_toggle_row_offset: None,
                        subagent_open_target: None,
                    }
                }
            };
            moved.message_index = message_index;
            moved.start_visual_row = absolute_visual_row;
            moved.logical_line_start = absolute_logical_row;
            moved
        };

        // Build flat indices from the (now settled) block. These are
        // usize/String/bool/Color — cheap to clone vs `Line` trees.
        // The first logical line of every message block is its role/tool
        // header (see `render_message_entry`); subsequent lines are body.
        const HEADER_LINE_INDEX: usize = 0;
        for (line_index, line) in block.lines.iter().enumerate() {
            let local_visual_start = block
                .logical_to_visual_offset
                .get(line_index)
                .copied()
                .unwrap_or(block.visual_lines.len());
            logical_line_visual_starts.push(absolute_visual_row + local_visual_start);
            line_texts.push(
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
            );
            line_is_header.push(line_index == HEADER_LINE_INDEX);
        }
        visual_line_backgrounds.extend(block.visual_lines.iter().map(|line| line.style.bg));

        absolute_visual_row += block.visual_lines.len();
        absolute_logical_row += block.lines.len();
        message_blocks.push(block);
    }

    #[cfg(debug_assertions)]
    {
        let _elapsed = _start.elapsed();
        // Suppress unused variable warning - used implicitly
        let _ = _elapsed;
    }

    TranscriptRenderCache {
        message_blocks,
        logical_line_visual_starts,
        line_texts,
        line_is_header,
        visual_line_backgrounds,
        total_lines: absolute_visual_row,
    }
}

fn paint_visible_line_backgrounds(
    frame: &mut Frame,
    area: Rect,
    transcript_cache: &TranscriptRenderCache,
    scroll_offset: usize,
) {
    let height = area.height as usize;
    for visible_row in 0..height {
        let visual_row = scroll_offset.saturating_add(visible_row);
        let Some(Some(bg)) = transcript_cache.visual_line_backgrounds.get(visual_row) else {
            continue;
        };
        frame.buffer_mut().set_style(
            Rect {
                x: area.x,
                y: area.y + visible_row as u16,
                width: area.width,
                height: 1,
            },
            Style::default().bg(*bg),
        );
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

fn line_plain_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
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
    full_text: &str,
    original_line: &Line<'static>,
) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let mut global_char_offset = 0;

    for wrapped_text in wrapped_lines {
        let line_text = wrapped_text.into_owned();
        let line_char_count = line_text.chars().count();

        // Align `global_char_offset` to the actual position of this visual
        // line's text within `full_text`. textwrap strips inter-word
        // whitespace at wrap boundaries, so each visual line's text is a
        // contiguous substring of `full_text` but not necessarily at
        // `global_char_offset` (the running count of chars in prior visual
        // lines). Without this alignment, style lookup via
        // `find_style_at_position(style_ranges, global_pos)` would drift by
        // the number of stripped whitespace chars, mis-coloring chars at span
        // boundaries.
        if let Some(pos) = find_substring_from(full_text, &line_text, global_char_offset) {
            global_char_offset = pos;
        }

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

pub(crate) fn wrap_line_to_visual_lines(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
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

    rebuild_lines_with_styles(wrapped_lines, &style_ranges, &full_text, line)
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
    let display_name = tool_display_name(tool);
    let mut header = format!("{toggle} {display_name}  {status}");
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

    if !tool.expanded {
        lines.push(Line::raw(""));
        return lines;
    }

    if let Some(command_text) = expanded_tool_command(tool) {
        lines.push(Line::styled(
            "  Command",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        for line in render_tool_detail_text(&command_text).lines() {
            lines.push(Line::styled(
                format!("    {}", sanitize_terminal_text(line)),
                Style::default().fg(theme.foreground),
            ));
        }
    }

    if tool.command.is_none() && !tool.args_preview.trim().is_empty() {
        let args_preview = display_tool_args_preview(tool);
        let args_preview = args_preview.trim();
        if args_preview.is_empty() {
            let detail_text = render_tool_detail_text(&tool.detail);
            let detail_text = detail_text.trim();
            if !detail_text.is_empty() {
                lines.push(Line::styled(
                    "  Output",
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::BOLD),
                ));
                append_tool_output_detail_lines(
                    &mut lines,
                    detail_text,
                    theme,
                    width,
                    theme.foreground,
                );
            }
            apply_expanded_tool_panel(&mut lines, theme, width);
            lines.push(Line::raw(""));
            return lines;
        }
        lines.push(Line::styled(
            "  Arguments",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        for line in args_preview.lines() {
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
    if tool.expanded {
        apply_expanded_tool_panel(&mut lines, theme, width);
    }
    lines.push(Line::raw(""));
    lines
}

fn apply_expanded_tool_panel(lines: &mut Vec<Line<'static>>, theme: &Theme, width: u16) {
    let bg = expanded_tool_background(theme);

    for line in lines.iter_mut() {
        line.style = style_with_panel_bg(line.style, bg);
        for span in &mut line.spans {
            span.style = style_with_panel_bg(span.style, bg);
        }
    }

    let panel_width = width.max(1) as usize;
    let spacer = || Line::styled(" ".repeat(panel_width), Style::default().bg(bg));
    lines.insert(0, spacer());
    lines.push(spacer());
}

#[cfg(test)]
fn line_display_width(line: &Line<'static>) -> usize {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn style_with_panel_bg(style: Style, bg: Color) -> Style {
    style.bg(bg)
}

fn expanded_tool_background(theme: &Theme) -> Color {
    match theme.background {
        Color::Rgb(_, _, _) if theme.is_light() => Color::Rgb(232, 248, 238),
        Color::Rgb(_, _, _) => Color::Rgb(18, 34, 28),
        Color::Indexed(_) if theme.is_light() => Color::Indexed(194),
        Color::Indexed(_) => Color::Indexed(22),
        Color::White => Color::LightGreen,
        Color::Black => Color::DarkGray,
        _ if theme.is_light() => Color::LightGreen,
        _ => Color::DarkGray,
    }
}

fn tool_display_name(tool: &ToolMessageState) -> String {
    if tool.tool == "bash" {
        if let Some(command) = collapsed_tool_command(tool) {
            return format!("bash: {command}");
        }
    }
    if let Some(command) = collapsed_tool_command(tool) {
        return format!("{}: {command}", tool.tool);
    }
    tool.tool.clone()
}

fn expanded_tool_command(tool: &ToolMessageState) -> Option<String> {
    tool.command
        .as_ref()
        .or(tool.command_preview.as_ref())
        .cloned()
        .or_else(|| command_from_args_preview(&tool.args_preview))
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
}

fn collapsed_tool_command(tool: &ToolMessageState) -> Option<String> {
    expanded_tool_command(tool)
        .map(|command| collapse_single_line(&render_tool_detail_text(&command)))
}

fn collapse_single_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ; ")
}

fn command_from_args_preview(args_preview: &str) -> Option<String> {
    let value: Value = serde_json::from_str(args_preview.trim()).ok()?;
    value
        .get("command")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn display_tool_args_preview(tool: &ToolMessageState) -> String {
    if tool.tool != "bash" {
        return render_tool_detail_text(&tool.args_preview);
    }

    let Ok(mut value) = serde_json::from_str::<Value>(tool.args_preview.trim()) else {
        return render_tool_detail_text(&tool.args_preview);
    };
    if let Value::Object(map) = &mut value {
        map.remove("timeout");
        if expanded_tool_command(tool).is_some() {
            map.remove("command");
        }
        if map.is_empty() {
            return String::new();
        }
    }

    serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| render_tool_detail_text(&tool.args_preview))
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

    if tool.expanded {
        apply_expanded_tool_panel(&mut lines, theme, width);
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
    incremental_markdown: Option<MarkdownIncrementalState>,
) -> (
    Vec<Line<'static>>,
    Vec<Vec<Line<'static>>>,
    Option<MarkdownIncrementalState>,
    Option<usize>,
) {
    #[cfg(debug_assertions)]
    let _start = std::time::Instant::now();
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

    // Thinking block occupies 1 header + N body + 1 blank line when present.
    // It sits between the role header and the markdown; when the thinking
    // length is unchanged across ticks it is part of the frozen prefix and
    // is moved (not re-rendered) from the previous tick's block.
    let thinking_len = message.thinking_content.len();
    let thinking_block_lines = if message.thinking_content.is_empty() {
        0
    } else {
        1 + message.thinking_content.lines().count() + 1
    };

    // Only reuse the incremental cache when the thinking block is unchanged
    // (a mismatch would misalign the frozen prefix). Otherwise pass None to
    // force a full fallback render for this tick.
    let usable_prev = match incremental_markdown {
        Some(ref s) if s.thinking_len() == thinking_len => incremental_markdown,
        _ => None,
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut wrapped: Vec<Vec<Line<'static>>> = Vec::new();

    // `push` a logical line together with its wrapped visual lines.
    let push = |lines: &mut Vec<Line<'static>>,
                wrapped: &mut Vec<Vec<Line<'static>>>,
                line: Line<'static>| {
        let w = wrap_line_to_visual_lines(&line, width);
        lines.push(line);
        wrapped.push(w);
    };

    // Markdown render (assistant with content). Determines whether the
    // incremental path applies — if so the header/thinking prefix is moved
    // from the previous tick's block and only the suffix is emitted here.
    //
    // The `!message.content.is_empty()` guard is intentional: during the
    // pure-thinking phase the thinking header shows a per-tick spinner
    // (`is_thinking` → "⭕️ ⠋ Thinking…"), so the block can never be frozen
    // (it must re-render each tick). More importantly, on the empty→non-
    // empty transition the header flips from the animated spinner to the
    // static "⭕️ Thought"; if the state from the thinking tick were reused
    // here, the incremental path would *move* the stale animated header
    // from the prev block and the spinner would freeze. Clearing the state
    // during thinking forces a full (cheap — no accumulated markdown yet)
    // render on the first content tick, which correctly switches the header
    // to "Thought". After that, subsequent content ticks reuse the state and
    // move the (now static) prefix.
    let mut new_markdown_state = None;
    let (md_lines, md_wrapped, md_move_count) = match message.role {
        MessageRole::Assistant if !message.content.is_empty() => {
            let result = render_markdown_incremental(usable_prev, &message.content, theme, width);
            let mut new_state = result.new_state;
            new_state.set_thinking_len(thinking_len);
            new_markdown_state = Some(new_state);
            (
                result.lines,
                result.wrapped,
                result.frozen_markdown_move_count,
            )
        }
        _ => (Vec::new(), Vec::new(), None),
    };
    let is_incremental = md_move_count.is_some();
    let frozen_prefix_line_count = md_move_count.map(|md_n| 1 + thinking_block_lines + md_n);

    // Role header + thinking block: skipped on the incremental path (they
    // are the frozen prefix moved from the previous block).
    if !is_incremental {
        push(
            &mut lines,
            &mut wrapped,
            Line::from(vec![
                Span::styled(
                    sanitize_terminal_text("▎ "),
                    Style::default().fg(indicator_color),
                ),
                Span::styled(role_label.to_string(), role_style),
                Span::styled(format!("  {timestamp}"), Style::default().fg(theme.muted)),
            ]),
        );

        if !message.thinking_content.is_empty() {
            let is_thinking =
                chat_is_loading && is_active_stream_message && message.content.is_empty();
            let thinking_header = if is_thinking {
                format!("  {} {loading_animation}", sanitize_terminal_text("⭕️"))
            } else {
                format!("  {} Thought", sanitize_terminal_text("⭕️"))
            };
            push(
                &mut lines,
                &mut wrapped,
                Line::styled(
                    thinking_header,
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::ITALIC),
                ),
            );
            let thinking_style = Style::default().fg(theme.muted).add_modifier(Modifier::DIM);
            for line in message.thinking_content.lines() {
                push(
                    &mut lines,
                    &mut wrapped,
                    Line::styled(
                        format!(
                            "  {} {}",
                            sanitize_terminal_text("│"),
                            sanitize_terminal_text(line)
                        ),
                        thinking_style,
                    ),
                );
            }
            push(&mut lines, &mut wrapped, Line::raw(""));
        }

        if show_stream_thinking {
            push(
                &mut lines,
                &mut wrapped,
                Line::styled(
                    format!("  {loading_animation}"),
                    Style::default().fg(theme.accent),
                ),
            );
        }
    }

    // Markdown / plain content.
    match message.role {
        MessageRole::Assistant if !message.content.is_empty() => {
            lines.extend(md_lines);
            wrapped.extend(md_wrapped);
        }
        _ => {
            for line in message.content.lines() {
                push(
                    &mut lines,
                    &mut wrapped,
                    Line::styled(format!("  {}", sanitize_terminal_text(line)), content_style),
                );
            }
        }
    }

    if message.is_streaming && !show_stream_thinking {
        push(
            &mut lines,
            &mut wrapped,
            Line::styled(
                format!("  {}", sanitize_terminal_text("▌")),
                Style::default().fg(theme.accent),
            ),
        );
    }
    push(&mut lines, &mut wrapped, Line::raw(""));
    #[cfg(debug_assertions)]
    {
        let elapsed = _start.elapsed();
        if elapsed > std::time::Duration::from_micros(200) && is_active_stream_message {
            tracing::debug!(
                "PERF render_standard_message_lines: {}µs, {} lines",
                elapsed.as_micros(),
                lines.len()
            );
        }
    }
    (lines, wrapped, new_markdown_state, frozen_prefix_line_count)
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

    apply_expanded_tool_panel(&mut lines, theme, width);
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
        lines.push(Line::from(vec![
            Span::styled("    agent_id: ", Style::default().fg(theme.foreground)),
            Span::styled(
                sanitize_terminal_text(&agent_id),
                Style::default().fg(theme.foreground),
            ),
            Span::raw("  "),
            Span::styled(
                sanitize_terminal_text("→"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "click to open",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
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
    use ratatui::text::{Line, Span};

    use crate::chat::{Message, ToolExecutionStatus, ToolExecutionUpdate};
    use crate::theme::Theme;

    use super::{
        apply_expanded_tool_panel, build_side_by_side_diff_rows, build_transcript_cache,
        diff_change_counts, expanded_tool_background, highlight_line_selection, line_display_width,
        parse_file_edit_args, parse_join_subagent_terminal, parse_spawn_subagent_agent_id,
        render_file_edit_tool_lines, render_message_entry, render_tool_message_lines,
        wrap_line_to_visual_lines,
    };
    use crate::app_state::CachedMessageRender;
    use crate::app_state::{MessageVisualBlock, TranscriptRenderCache};

    /// Regression test for the "missing last line" bug. `build_transcript_cache`
    /// must derive `total_lines` from the same `wrap_line_to_visual_lines` call
    /// that produces `visual_lines`, so the two stay in lock-step and the
    /// stick-to-bottom viewport never slices off the trailing visual line(s).
    /// Long paths/URLs force textwrap to emit more visual lines than a naive
    /// character-based `div_ceil` predictor would, which previously left the
    /// trailing line stuck below the viewport.
    #[test]
    fn build_transcript_cache_keeps_total_lines_in_sync_with_visual_lines() {
        // Wrap width that triggers the predictor/wrap mismatch: the long
        // synthetic path (45 chars in parens, > width) forces textwrap to emit
        // 3 visual lines while a character-based predictor only sees 2.
        let wrap_width: u16 = 40;
        let mismatch_text =
            "Session snapshot saved: name (/tmp/xiaoo-test/sessions/snapshot-name.json)";
        let mismatch_line = Line::from(mismatch_text);
        let lines = vec![Line::from("System header"), mismatch_line, Line::raw("")];
        let wrapped_lines: Vec<Vec<Line<'static>>> = lines
            .iter()
            .map(|line| wrap_line_to_visual_lines(line, wrap_width))
            .collect();
        let render = CachedMessageRender {
            width: wrap_width,
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            wrapped_lines: Some(wrapped_lines),
            lines,
            frozen_prefix_line_count: None,
        };

        let cache = build_transcript_cache(None, vec![Some(render)]);

        // `total_lines` must equal the actual number of visual lines across all
        // blocks, so stick_to_bottom's `scroll_offset = total_lines -
        // inner_height` never points past the last real visual line.
        let actual_visual_lines: usize = cache
            .message_blocks
            .iter()
            .map(|b| b.visual_lines.len())
            .sum();
        assert_eq!(
            cache.total_lines, actual_visual_lines,
            "total_lines must match the sum of block visual_lines.len() so \
             stick_to_bottom scroll_offset (= total_lines - inner_height) \
             never points past the last actual visual line"
        );
        let flat = cache.collect_visible_visual_lines(0, cache.total_lines);
        // The last non-empty visual line (before the trailing empty spacer)
        // must be the path tail that textwrap split off — the exact line that
        // used to be hidden below the viewport.
        let last_content_visual: String = flat
            .iter()
            .rev()
            .skip(1) // skip the trailing empty spacer
            .next()
            .expect("cache should have at least one content line")
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // The tail must be a substring of the original text, proving it
        // wasn't dropped.
        assert!(
            mismatch_text.contains(&last_content_visual),
            "last content visual line {last_content_visual:?} must be a substring of the original text"
        );
    }

    /// Regression test for the `rebuild_lines_with_styles` style-offset bug.
    ///
    /// textwrap strips inter-word whitespace at wrap boundaries, so each
    /// visual line's text is a contiguous substring of the original but not
    /// necessarily at a contiguous running offset. Before the fix,
    /// `global_char_offset` was advanced by `line_char_count` only, drifting
    /// past the stripped spaces and looking up the wrong style for chars at
    /// span boundaries. After the fix, `find_substring_from` realigns
    /// `global_char_offset` to each visual line's actual position in the
    /// original text.
    #[test]
    fn rebuild_lines_with_styles_preserves_style_positions_after_whitespace_stripping() {
        // Three styled spans separated by single spaces: "AAA BBB CCC".
        // At width 4, textwrap emits ["AAA", "BBB", "CCC"] (spaces stripped).
        // Without the fix, BBB's chars would get the style at offsets 3/4/5
        // (space + first two italic chars) instead of 4/5/6 (italic ×3).
        let bold = Style::default().fg(Color::Red);
        let italic = Style::default().fg(Color::Green);
        let underline = Style::default().fg(Color::Blue);

        let line = Line::from(vec![
            Span::styled("AAA", bold),
            Span::raw(" "),
            Span::styled("BBB", italic),
            Span::raw(" "),
            Span::styled("CCC", underline),
        ]);

        let wrapped = wrap_line_to_visual_lines(&line, 4);
        assert_eq!(
            wrapped.len(),
            3,
            "textwrap at width 4 must split into 3 visual lines"
        );

        // Collect (char, style.fg) pairs from each visual line.
        let collect_fg = |l: &Line<'_>| -> Vec<Option<Color>> {
            l.spans
                .iter()
                .flat_map(|span| span.content.chars().map(move |_| span.style.fg))
                .collect()
        };

        // v0 = "AAA" — all red (bold).
        assert_eq!(
            collect_fg(&wrapped[0]),
            vec![Some(Color::Red), Some(Color::Red), Some(Color::Red)],
            "v0 must be entirely red"
        );
        // v1 = "BBB" — all green (italic). Before the fix, the first char
        // would inherit the previous span's style (None from the raw space)
        // or shift forward by one.
        assert_eq!(
            collect_fg(&wrapped[1]),
            vec![Some(Color::Green), Some(Color::Green), Some(Color::Green)],
            "v1 must be entirely green — proves global_char_offset was realigned"
        );
        // v2 = "CCC" — all blue (underline).
        assert_eq!(
            collect_fg(&wrapped[2]),
            vec![Some(Color::Blue), Some(Color::Blue), Some(Color::Blue)],
            "v2 must be entirely blue"
        );
    }

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
    fn bash_tool_header_includes_command_when_collapsed() {
        let theme = Theme::detect();
        let message = Message::tool_event(ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "bash".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "command": "cargo test -p xiaoo-endside",
                "timeout": 120000
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: "full output".to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: Some(0),
            duration_ms: Some(10),
            file_change: None,
        });
        let tool = message
            .tool_state
            .as_ref()
            .expect("tool message should carry tool state");

        let lines = render_tool_message_lines(&message, tool, Color::Green, &theme, 80);
        let rendered_text = rendered_lines_text(&lines);

        assert!(rendered_text.contains("bash: cargo test -p xiaoo-endside  done"));
        assert!(!rendered_text.contains("Command"));
        assert!(!rendered_text.contains("full output"));
        assert!(!rendered_text.contains("timeout"));
    }

    #[test]
    fn expanded_bash_tool_filters_timeout_and_decodes_escaped_output() {
        let theme = Theme::detect();
        let message = Message::tool_event(ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "bash".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "command": "printf 'a\\nb'",
                "cwd": "/tmp/work",
                "timeout": 120000
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: "line1\\nline2\\tindented".to_string(),
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
        let rendered_text = rendered_lines_text(&lines);

        assert!(rendered_text.contains("Command"));
        assert!(rendered_text.contains("printf 'a"));
        assert!(rendered_text.contains("Arguments"));
        assert!(rendered_text.contains("\"cwd\": \"/tmp/work\""));
        assert!(!rendered_text.contains("\"timeout\""));
        assert!(rendered_text.contains("line1"));
        assert!(rendered_text.contains("line2\tindented"));
    }

    #[test]
    fn expanded_tool_lines_use_subtle_background() {
        let theme = Theme::detect();
        let message = Message::tool_event(ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "bash".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "command": "date"
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: "Mon Jun 29".to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: Some(0),
            duration_ms: None,
            file_change: None,
        });
        let mut tool = message
            .tool_state
            .clone()
            .expect("tool message should carry tool state");

        let collapsed_lines = render_tool_message_lines(&message, &tool, Color::Green, &theme, 80);
        assert!(collapsed_lines.iter().all(|line| line.style.bg.is_none()));

        tool.expanded = true;
        let expanded_lines = render_tool_message_lines(&message, &tool, Color::Green, &theme, 80);
        let bg = Some(expanded_tool_background(&theme));
        assert_ne!(bg, Some(theme.background));
        assert_ne!(bg, Some(theme.assistant_message_bg));
        let panel_lines = expanded_lines
            .iter()
            .take_while(|line| line_display_width(line) > 0)
            .collect::<Vec<_>>();
        assert!(panel_lines.len() >= 4);
        assert!(panel_lines.iter().all(|line| line.style.bg == bg));
        assert!(line_display_width(panel_lines[0]) >= 80);
        assert!(line_display_width(panel_lines[panel_lines.len() - 1]) >= 80);
        assert!(rendered_line_text(panel_lines[0]).trim().is_empty());
        assert!(rendered_line_text(panel_lines[panel_lines.len() - 1])
            .trim()
            .is_empty());
    }

    #[test]
    fn expanded_tool_panel_overrides_nested_markdown_backgrounds() {
        let theme = Theme::detect();
        let mut lines = vec![Line::from(vec![Span::styled(
            "`code`",
            Style::default().fg(theme.foreground).bg(theme.code_bg),
        )])];

        apply_expanded_tool_panel(&mut lines, &theme, 80);

        let bg = Some(expanded_tool_background(&theme));
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .all(|span| span.style.bg.is_none() || span.style.bg == bg));
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.content == "`code`" && span.style.bg == bg));
    }

    #[test]
    fn tool_toggle_row_tracks_expanded_panel_spacer() {
        let theme = Theme::detect();
        let mut message = Message::tool_event(ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "bash".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "command": "date"
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: "ok".to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: Some(0),
            duration_ms: None,
            file_change: None,
        });

        let (collapsed, _state) =
            render_message_entry(&message, &theme, 80, false, false, "", None);
        assert_eq!(collapsed.tool_toggle_row_offset, Some(1));

        message
            .tool_state
            .as_mut()
            .expect("tool state should exist")
            .expanded = true;
        let (expanded, _state) = render_message_entry(&message, &theme, 80, false, false, "", None);
        assert_eq!(expanded.tool_toggle_row_offset, Some(2));
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

    fn rendered_lines_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(rendered_line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rendered_line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn make_bench_render(
        lines: &[Line<'static>],
        width: u16,
        cache_wrapped: bool,
    ) -> CachedMessageRender {
        let wrapped_lines = if cache_wrapped {
            Some(
                lines
                    .iter()
                    .map(|l| wrap_line_to_visual_lines(l, width))
                    .collect(),
            )
        } else {
            None
        };
        CachedMessageRender {
            width,
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            wrapped_lines,
            lines: lines.to_vec(),
            frozen_prefix_line_count: None,
        }
    }

    /// Build a `(renders_all_dirty, renders_partial)` pair for a bench scenario.
    ///
    /// `renders_all_dirty`: every slot is `Some` (freshly rendered) — used to
    /// build the prev cache and to measure the full-rebuild path.
    /// `renders_partial`: only the last `dirty_count` slots are `Some`; the
    /// rest are `None` so their blocks MOVE from the prev cache. This models a
    /// streaming tick where only the tail (and possibly a few recently-settled
    /// messages) changed.
    fn make_bench_renders(
        num_messages: usize,
        content_lines: &[Line<'static>],
        width: u16,
        dirty_count: usize,
    ) -> (
        Vec<Option<CachedMessageRender>>,
        Vec<Option<CachedMessageRender>>,
    ) {
        let all_dirty: Vec<Option<CachedMessageRender>> = (0..num_messages)
            .map(|_| Some(make_bench_render(content_lines, width, true)))
            .collect();
        let dirty_tail_start = num_messages.saturating_sub(dirty_count);
        let partial: Vec<Option<CachedMessageRender>> = (0..num_messages)
            .map(|i| {
                if i >= dirty_tail_start {
                    Some(make_bench_render(content_lines, width, true))
                } else {
                    None
                }
            })
            .collect();
        (all_dirty, partial)
    }

    /// Time the three `build_transcript_cache` paths and return per-run µs:
    /// `(full_no_prev, incremental_step, full_with_prev)`.
    ///
    /// - `full_no_prev` (A): cold cache, every message dirty.
    /// - `incremental_step` (B-A): pure incremental cost, isolated by
    ///   subtracting A from B (B includes building the prev cache each
    ///   iteration, which is A's cost, so B-A cancels it).
    /// - `full_with_prev` (C): prev exists but every message is dirty
    ///   (width/theme-change style) — measures the `Option::take` overhead
    ///   on the non-moving path.
    ///
    /// All three include the cost of cloning the renders vec each iteration
    /// (`build_transcript_cache` takes it by value); this is a constant
    /// bias across paths and scenarios, so ratios and trends remain valid.
    fn time_bench_paths(
        all_dirty: &[Option<CachedMessageRender>],
        partial: &[Option<CachedMessageRender>],
        num_warmup: u32,
        num_measure: u32,
    ) -> (u128, u128, u128) {
        use std::time::Instant;

        for _ in 0..num_warmup {
            let _ = build_transcript_cache(None, all_dirty.to_vec());
            let prev = build_transcript_cache(None, all_dirty.to_vec());
            let _ = build_transcript_cache(Some(prev), partial.to_vec());
        }
        let a_start = Instant::now();
        for _ in 0..num_measure {
            let _ = build_transcript_cache(None, all_dirty.to_vec());
        }
        let a_us = a_start.elapsed().as_micros() / num_measure as u128;
        let b_start = Instant::now();
        for _ in 0..num_measure {
            let prev = build_transcript_cache(None, all_dirty.to_vec());
            let _ = build_transcript_cache(Some(prev), partial.to_vec());
        }
        let b_us = b_start.elapsed().as_micros() / num_measure as u128;
        let c_start = Instant::now();
        for _ in 0..num_measure {
            let prev = build_transcript_cache(None, all_dirty.to_vec());
            let _ = build_transcript_cache(Some(prev), all_dirty.to_vec());
        }
        let c_us = c_start.elapsed().as_micros() / num_measure as u128;
        (a_us, b_us.saturating_sub(a_us), c_us)
    }

    /// Short content fixture: each logical line fits in one visual line at
    /// the bench widths, so wrapping is a no-op. Isolates the per-message /
    /// per-logical-line overhead from the wrap cost.
    fn short_content(lines_per_message: usize) -> Vec<Line<'static>> {
        (0..lines_per_message)
            .map(|i| Line::from(format!("Line {i}")))
            .collect()
    }

    /// Long content fixture: each logical line wraps to several visual lines
    /// at width 80, exercising `wrap_line_to_visual_lines` and the flat-index
    /// rebuild over a larger `visual_lines` vec.
    fn long_content(lines_per_message: usize) -> Vec<Line<'static>> {
        (0..lines_per_message)
            .map(|i| {
                Line::from(format!(
                    "Lorem ipsum dolor sit amet consectetur adipiscing elit adipiscing elit {i}"
                ))
            })
            .collect()
    }

    /// Microbenchmark for incremental `build_transcript_cache`.
    ///
    /// Compares three paths:
    ///  - A) Full rebuild, no prev (`None` for every message) — the
    ///    worst-case cost when the cache is cold (first tick / post-switch).
    ///  - B) Incremental rebuild: a prev cache exists, only the last message
    ///    is dirty, the rest move from prev. This is the streaming hot path.
    ///  - C) Full rebuild with a prev cache present (e.g. width change) —
    ///    prev is taken but every message is dirty, so nothing moves.
    ///
    /// B includes the cost of building the prev cache each iteration (it is
    /// consumed by `build_transcript_cache`), so `B - A` isolates the pure
    /// incremental step. The move-verification assertion checks that non-dirty
    /// blocks share the same `visual_lines` allocation as the prev cache
    /// (i.e. moved, not cloned).
    #[test]
    fn build_transcript_cache_bench() {
        // Bench fixture: 50 messages × 5 logical lines, wrapped at width 80.
        let num_messages = 50usize;
        let lines_per_message = 5usize;
        // Terminal width used for wrapping throughout the bench.
        let bench_width: u16 = 80;
        // Warmup iterations to stabilise timings (JIT-less, but primes caches).
        let num_warmup_runs = 10u32;
        // Measured iterations averaged per path — large enough to dwarf the
        // `Instant::now()` overhead at µs granularity.
        let num_measure_runs = 100u32;

        let content_lines = long_content(lines_per_message);

        // Sanity: wrap_line_to_visual_lines works from test context.
        let test_wrapped = wrap_line_to_visual_lines(&content_lines[0], bench_width);
        assert!(
            !test_wrapped.is_empty(),
            "wrap_line_to_visual_lines should produce at least one visual line"
        );
        let test_miss = make_bench_render(&content_lines, bench_width, false);
        assert!(
            test_miss.wrapped_lines.is_none(),
            "cache_wrapped=false should produce None"
        );
        let test_hit = make_bench_render(&content_lines, bench_width, true);
        assert!(
            test_hit.wrapped_lines.is_some(),
            "cache_wrapped=true should produce Some(wrapped_lines)"
        );

        let (renders_all_dirty, renders_incremental) =
            make_bench_renders(num_messages, &content_lines, bench_width, 1);
        for (i, r) in renders_all_dirty.iter().enumerate() {
            assert!(r.is_some(), "render {i} should be Some (dirty)");
        }

        let (a_us, incremental_step_us, c_us) = time_bench_paths(
            &renders_all_dirty,
            &renders_incremental,
            num_warmup_runs,
            num_measure_runs,
        );

        // Move verification: a non-dirty block's `visual_lines` must keep the
        // same heap allocation across the incremental rebuild (moved, not
        // cloned). `Vec::as_ptr` is stable across moves of the owning Vec.
        // Block 0 is non-dirty (it is not the dirty tail), so it must be moved.
        let moved_block_idx = 0usize;
        let dirty_block_idx = num_messages - 1;
        let prev_for_check = build_transcript_cache(None, renders_all_dirty.clone());
        let moved_ptr = prev_for_check.message_blocks[moved_block_idx]
            .visual_lines
            .as_ptr();
        let new_cache = build_transcript_cache(Some(prev_for_check), renders_incremental.clone());
        assert_eq!(
            new_cache.message_blocks[moved_block_idx]
                .visual_lines
                .as_ptr(),
            moved_ptr,
            "incremental rebuild must move (not clone) non-dirty visual_lines — \
             pointer should be unchanged"
        );
        // The dirty (last) block, by contrast, is freshly built.
        assert_ne!(
            new_cache.message_blocks[dirty_block_idx]
                .visual_lines
                .as_ptr(),
            moved_ptr,
            "dirty block should be a fresh allocation"
        );

        eprintln!();
        eprintln!("=== build_transcript_cache microbenchmark (incremental) ===");
        eprintln!(
            "  Messages: {num_messages}, ~{lines_per_message} lines each, width {bench_width}"
        );
        eprintln!(
            "  A) Full rebuild (no prev):       {a_us:>6} µs avg (over {num_measure_runs} runs)"
        );
        eprintln!("  B-A) Pure incremental step:      {incremental_step_us:>6} µs avg (1 dirty, prev exists)");
        eprintln!("  C) Full rebuild with prev:       {c_us:>6} µs avg (over {num_measure_runs} runs, incl prev-build)");
        eprintln!(
            "  Speedup A vs (B-A):              {:.1}×",
            if incremental_step_us > 0 {
                a_us as f64 / incremental_step_us as f64
            } else {
                f64::INFINITY
            }
        );
        eprintln!();

        // The pure incremental step must be cheaper than a full rebuild.
        assert!(
            incremental_step_us < a_us,
            "incremental step ({}µs) should be cheaper than full rebuild ({}µs)",
            incremental_step_us,
            a_us
        );
    }

    /// Scalability matrix for `build_transcript_cache`.
    ///
    /// Sweeps four axes and prints one row per scenario, holding the others
    /// fixed at the bench defaults (50 messages, long content, width 80, 1
    /// dirty tail). Each row reports `full` (cold rebuild) and `incr` (the
    /// pure incremental step, B-A) in µs, plus the speedup ratio. A high
    /// `full/incr` ratio is the goal; it should stay well above 1× across
    /// the matrix except the 100%-dirty degenerate case.
    ///
    /// Axes:
    ///  1. Message count (10 / 50 / 200) at 1 dirty — does the incremental
    ///     step stay flat (move-only) while full rebuild grows linearly?
    ///  2. Dirty count (1 / 10% / 50% / 100%) at 50 messages — how does the
    ///     incremental step degrade as more messages re-wrap? At 100% it
    ///     approaches full rebuild (width-change worst case).
    ///  3. Terminal width (40 / 80 / 120) at 50 messages, 1 dirty — wider
    ///     terminals wrap less, so fewer visual lines per message.
    ///  4. Content shape (short / long) at 50 messages, 1 dirty — short
    ///     content skips wrapping, isolating per-logical-line overhead.
    #[test]
    fn build_transcript_cache_scalability() {
        // Fewer runs than the main bench: the matrix has many scenarios and
        // we care about the trend, not single-scenario precision.
        let num_warmup_runs = 5u32;
        let num_measure_runs = 30u32;
        let default_messages = 50usize;
        let default_lines = 5usize;
        let default_width: u16 = 80;
        let default_dirty = 1usize;

        let mut rows: Vec<(String, u128, u128, f64)> = Vec::new();

        // Axis 1: message count, 1 dirty tail, long content, width 80.
        for &num_messages in &[10usize, 50usize, 200usize] {
            let content = long_content(default_lines);
            let (all, partial) =
                make_bench_renders(num_messages, &content, default_width, default_dirty);
            let (full, incr, _) =
                time_bench_paths(&all, &partial, num_warmup_runs, num_measure_runs);
            let speedup = if incr > 0 {
                full as f64 / incr as f64
            } else {
                f64::INFINITY
            };
            rows.push((
                format!("msgs={num_messages:>3} dirty=1"),
                full,
                incr,
                speedup,
            ));
        }

        // Axis 2: dirty count (tail), 50 messages, long content, width 80.
        for &dirty in &[1usize, 5usize, 25usize, 50usize] {
            let content = long_content(default_lines);
            let (all, partial) =
                make_bench_renders(default_messages, &content, default_width, dirty);
            let (full, incr, _) =
                time_bench_paths(&all, &partial, num_warmup_runs, num_measure_runs);
            let speedup = if incr > 0 {
                full as f64 / incr as f64
            } else {
                f64::INFINITY
            };
            let pct = (dirty * 100) / default_messages;
            rows.push((
                format!("msgs=50 dirty={dirty:>2} ({pct}%)"),
                full,
                incr,
                speedup,
            ));
        }

        // Axis 3: terminal width, 50 messages, 1 dirty, long content.
        for &width in &[40u16, 80u16, 120u16] {
            let content = long_content(default_lines);
            let (all, partial) =
                make_bench_renders(default_messages, &content, width, default_dirty);
            let (full, incr, _) =
                time_bench_paths(&all, &partial, num_warmup_runs, num_measure_runs);
            let speedup = if incr > 0 {
                full as f64 / incr as f64
            } else {
                f64::INFINITY
            };
            rows.push((format!("msgs=50 dirty=1 w={width:>3}"), full, incr, speedup));
        }

        // Axis 4: content shape, 50 messages, 1 dirty, width 80.
        for (label, content) in [
            ("short", short_content(default_lines)),
            ("long", long_content(default_lines)),
        ] {
            let (all, partial) =
                make_bench_renders(default_messages, &content, default_width, default_dirty);
            let (full, incr, _) =
                time_bench_paths(&all, &partial, num_warmup_runs, num_measure_runs);
            let speedup = if incr > 0 {
                full as f64 / incr as f64
            } else {
                f64::INFINITY
            };
            rows.push((format!("msgs=50 dirty=1 {label}"), full, incr, speedup));
        }

        eprintln!();
        eprintln!("=== build_transcript_cache scalability matrix ===");
        eprintln!(
            "  (full = cold rebuild µs, incr = pure incremental step µs, speedup = full/incr)"
        );
        eprintln!(
            "  {:<22} {:>10} {:>10} {:>10}",
            "scenario", "full", "incr", "speedup"
        );
        eprintln!("  {:-<22} {:-<10} {:-<10} {:-<10}", "", "", "", "");
        for (label, full, incr, speedup) in &rows {
            eprintln!(
                "  {:<22} {:>8}µs {:>8}µs {:>8.1}×",
                label, full, incr, speedup
            );
        }
        eprintln!();

        // The incremental step must beat full rebuild in every scenario
        // except the 100%-dirty degenerate case (where they are equal by
        // construction: nothing moves). Assert the non-degenerate cases.
        for (label, full, incr, _speedup) in &rows {
            if label.contains("dirty=50 (100%)") {
                // 100% dirty = width-change worst case; incr ≈ full is expected.
                continue;
            }
            assert!(
                incr < full,
                "scenario {label}: incremental step ({incr}µs) must be cheaper \
                 than full rebuild ({full}µs)",
            );
        }
    }

    /// End-to-end test for the incremental block-move path in
    /// `build_transcript_cache`: a dirty render with
    /// `frozen_prefix_line_count = Some(n)` must move the first `n` logical
    /// (and their visual) lines from the previous tick's block and append
    /// the suffix, producing the same result as a full render.
    #[test]
    fn build_transcript_cache_incremental_moves_frozen_prefix() {
        let width: u16 = 80;

        // Previous tick: a full block [header, md1, md2, cursor, blank].
        // Each short line wraps to exactly one visual line. The Vecs are
        // constructed with spare capacity so the incremental path's
        // `truncate` + `extend` does not reallocate — letting us verify the
        // frozen prefix is MOVED (same heap allocation), not cloned.
        let mut prev_lines: Vec<Line<'static>> = Vec::with_capacity(32);
        prev_lines.push(Line::from("header"));
        prev_lines.push(Line::from("md1"));
        prev_lines.push(Line::from("md2"));
        prev_lines.push(Line::from("▌"));
        prev_lines.push(Line::raw(""));
        let mut prev_visuals: Vec<Line<'static>> = Vec::with_capacity(32);
        for l in &prev_lines {
            prev_visuals.extend(wrap_line_to_visual_lines(l, width));
        }
        let prev_l2v: Vec<usize> = {
            let mut v = Vec::new();
            let mut acc = 0;
            for l in &prev_lines {
                let w = wrap_line_to_visual_lines(l, width);
                v.push(acc);
                acc += w.len();
            }
            v
        };
        let line_texts: Vec<String> = prev_lines.iter().map(rendered_line_text).collect();
        let line_is_header: Vec<bool> = (0..prev_lines.len()).map(|i| i == 0).collect();
        let total_lines = prev_visuals.len();
        let visual_line_backgrounds = vec![None; total_lines];

        let prev_block = MessageVisualBlock {
            message_index: 0,
            start_visual_row: 0,
            logical_line_start: 0,
            lines: prev_lines,
            visual_lines: prev_visuals,
            logical_to_visual_offset: prev_l2v.clone(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
        };
        // Save the block's heap pointers before it is moved into the cache.
        // Vec::as_ptr is stable across moves and `truncate` (no realloc), so
        // the new block's moved prefix must share these exact allocations.
        let prev_lines_ptr = prev_block.lines.as_ptr();
        let prev_visuals_ptr = prev_block.visual_lines.as_ptr();
        let prev_cache = TranscriptRenderCache {
            message_blocks: vec![prev_block],
            logical_line_visual_starts: prev_l2v,
            line_texts,
            line_is_header,
            visual_line_backgrounds,
            total_lines,
        };

        // This tick: frozen prefix = [header, md1, md2] (3 lines).
        // Suffix = [md3, cursor, blank] (newly rendered).
        let suffix_lines: Vec<Line<'static>> =
            vec![Line::from("md3"), Line::from("▌"), Line::raw("")];
        let suffix_wrapped: Vec<Vec<Line<'static>>> = suffix_lines
            .iter()
            .map(|l| wrap_line_to_visual_lines(l, width))
            .collect();
        let render = CachedMessageRender {
            width,
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            wrapped_lines: Some(suffix_wrapped),
            lines: suffix_lines,
            frozen_prefix_line_count: Some(3),
        };

        let new_cache = build_transcript_cache(Some(prev_cache), vec![Some(render)]);
        let block = &new_cache.message_blocks[0];

        // Combined logical lines = moved prefix + suffix.
        assert_eq!(block.lines.len(), 6, "prefix(3) + suffix(3)");
        assert_eq!(rendered_line_text(&block.lines[0]), "header");
        assert_eq!(rendered_line_text(&block.lines[1]), "md1");
        assert_eq!(rendered_line_text(&block.lines[2]), "md2");
        assert_eq!(rendered_line_text(&block.lines[3]), "md3");
        assert_eq!(rendered_line_text(&block.lines[4]), "▌");
        assert_eq!(rendered_line_text(&block.lines[5]), "");

        // The frozen prefix lines must be MOVED from the prev block (same
        // heap allocation), not cloned.
        assert_eq!(
            block.lines.as_ptr(),
            prev_lines_ptr,
            "frozen prefix lines must be moved (same allocation), not cloned"
        );
        assert_eq!(
            block.visual_lines.as_ptr(),
            prev_visuals_ptr,
            "frozen prefix visual_lines must be moved (same allocation)"
        );

        // logical_to_visual_offset: 6 logical lines, each → 1 visual line.
        assert_eq!(block.logical_to_visual_offset, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(block.visual_lines.len(), 6);
        assert_eq!(new_cache.total_lines, 6);
    }

    /// A block-move where the suffix wraps to multiple visual lines must
    /// produce correct `logical_to_visual_offset` entries that continue from
    /// the frozen prefix's visual count (not restart at 0).
    #[test]
    fn build_transcript_cache_incremental_offset_continues_from_prefix() {
        let width: u16 = 10;

        // Prev: [header, long_line_that_wraps_to_2] → 3 visual lines total.
        let prev_lines: Vec<Line<'static>> =
            vec![Line::from("hdr"), Line::from("a very long line that wraps")];
        let prev_wrapped: Vec<Vec<Line<'static>>> = prev_lines
            .iter()
            .map(|l| wrap_line_to_visual_lines(l, width))
            .collect();
        let prev_visuals: Vec<Line<'static>> = prev_wrapped.iter().flatten().cloned().collect();
        let prev_l2v: Vec<usize> = {
            let mut v = Vec::new();
            let mut acc = 0;
            for wl in &prev_wrapped {
                v.push(acc);
                acc += wl.len();
            }
            v
        };
        let prefix_visual_count = prev_l2v[1]; // visual count for header only
        let prev_block = MessageVisualBlock {
            message_index: 0,
            start_visual_row: 0,
            logical_line_start: 0,
            lines: prev_lines.clone(),
            visual_lines: prev_visuals.clone(),
            logical_to_visual_offset: prev_l2v.clone(),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
        };
        let prev_cache = TranscriptRenderCache {
            message_blocks: vec![prev_block],
            logical_line_visual_starts: prev_l2v.clone(),
            line_texts: prev_lines.iter().map(rendered_line_text).collect(),
            line_is_header: vec![true, false],
            visual_line_backgrounds: vec![None; prev_visuals.len()],
            total_lines: prev_visuals.len(),
        };

        // Freeze only the header (1 line); suffix = another long line.
        let suffix_lines: Vec<Line<'static>> =
            vec![Line::from("another long wrapping suffix line")];
        let suffix_wrapped: Vec<Vec<Line<'static>>> = suffix_lines
            .iter()
            .map(|l| wrap_line_to_visual_lines(l, width))
            .collect();
        let suffix_visual_count: usize = suffix_wrapped[0].len();
        let render = CachedMessageRender {
            width,
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            wrapped_lines: Some(suffix_wrapped),
            lines: suffix_lines,
            frozen_prefix_line_count: Some(1),
        };

        let new_cache = build_transcript_cache(Some(prev_cache), vec![Some(render)]);
        let block = &new_cache.message_blocks[0];

        assert_eq!(block.lines.len(), 2);
        assert_eq!(rendered_line_text(&block.lines[0]), "hdr");
        assert_eq!(
            rendered_line_text(&block.lines[1]),
            "another long wrapping suffix line"
        );

        // l2v[0] = 0 (header), l2v[1] = prefix_visual_count (suffix starts
        // after the header's visual lines).
        assert_eq!(block.logical_to_visual_offset.len(), 2);
        assert_eq!(block.logical_to_visual_offset[0], 0);
        assert_eq!(
            block.logical_to_visual_offset[1], prefix_visual_count,
            "suffix offset must continue from the frozen prefix's visual count"
        );
        assert_eq!(
            block.visual_lines.len(),
            prefix_visual_count + suffix_visual_count
        );
    }
}
