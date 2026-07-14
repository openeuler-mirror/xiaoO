use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
    },
    Frame,
};

use crate::app::App;
use crate::app_state::{ApiKeyDialogState, InputMode, SandboxDialog};
use crate::interaction_prompt::{interaction_prompt_outer_height, render_interaction_prompt};
use crate::provider_dialog::ProviderDialog;
use crate::remote_sessions_service::{
    daemon_display, format_remote_time, RemoteSessionDialog, RemoteSessionDialogEntry,
    RemoteSessionDialogMode,
};
use crate::services::turn_delete::DeleteDialog;
use crate::session_snapshot_service::{format_snapshot_time, SessionSnapshotDialog};

use super::utils::{line_prefix_width, sanitize_terminal_text};

/// Flatten newlines and truncate `text` to fit within `max_width` terminal columns,
/// appending "..." when truncated.
fn truncate_to_width(text: &str, max_width: u16) -> String {
    let flattened = text.replace('\n', " ");
    if flattened.is_empty() || max_width == 0 {
        return String::new();
    }
    let max = max_width as usize;
    if max <= 3 {
        return ".".repeat(max);
    }
    let full_width: usize = flattened.chars().map(char_display_width).sum();
    if full_width <= max {
        return flattened;
    }
    let target = max - 3;
    let mut width = 0;
    let mut end = 0;
    for (i, c) in flattened.char_indices() {
        let cw = char_display_width(c);
        if width + cw > target {
            break;
        }
        width += cw;
        end = i + c.len_utf8();
    }
    format!("{}...", &flattened[..end])
}

fn char_display_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

fn expand_popup_area(area: Rect, bounds: Rect, margin: u16) -> Rect {
    let left = area.x.saturating_sub(margin).max(bounds.x);
    let top = area.y.saturating_sub(margin).max(bounds.y);
    let right = area
        .x
        .saturating_add(area.width)
        .saturating_add(margin)
        .min(bounds.x.saturating_add(bounds.width));
    let bottom = area
        .y
        .saturating_add(area.height)
        .saturating_add(margin)
        .min(bounds.y.saturating_add(bounds.height));

    Rect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    }
}

fn render_popup_backdrop(frame: &mut Frame, area: Rect, bounds: Rect, bg: ratatui::style::Color) {
    let backdrop = expand_popup_area(area, bounds, 1);
    frame.render_widget(Clear, backdrop);
    frame.render_widget(Block::default().style(Style::default().bg(bg)), backdrop);
}

impl App {
    pub(crate) fn render_pending_turns(&self, frame: &mut Frame, input_area: Rect, bounds: Rect) {
        let pending = &self.state.chat_state.pending_turns;
        if pending.is_empty() || input_area.y <= bounds.y {
            return;
        }

        let visible_count = if pending.len() > 3 { 2 } else { pending.len() };
        let skipped = pending.len().saturating_sub(visible_count);
        let line_count = visible_count + usize::from(skipped > 0);
        let height = (line_count as u16 + 2).min(input_area.y.saturating_sub(bounds.y));
        if height == 0 {
            return;
        }

        let available_width = bounds.width.min(input_area.width).max(1);
        let width = if available_width >= 32 {
            available_width.min(56)
        } else {
            available_width
        };
        let x = bounds
            .x
            .saturating_add(bounds.width)
            .saturating_sub(width.saturating_add(2))
            .max(bounds.x);
        let y = input_area
            .y
            .saturating_sub(height.saturating_add(1))
            .max(bounds.y);
        let area = Rect {
            x,
            y,
            width,
            height,
        };
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.muted))
            .title(" 待发送 ")
            .style(Style::default().bg(self.state.theme.input_bg));

        let inner_width = width.saturating_sub(4) as usize;
        let mut lines = Vec::new();
        if skipped > 0 {
            lines.push(Line::styled(
                format!("  ... 还有 {skipped} 条更早输入"),
                Style::default().fg(self.state.theme.muted),
            ));
        }

        for queued in pending.iter().skip(skipped) {
            let prompt = one_line_preview(&queued.prompt, inner_width.saturating_sub(5));
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(self.state.theme.muted)),
                Span::styled(prompt, Style::default().fg(self.state.theme.muted)),
                Span::styled("  ", Style::default().fg(self.state.theme.muted)),
                Span::styled(
                    sanitize_terminal_text("●"),
                    Style::default()
                        .fg(self.state.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    pub(crate) fn render_interaction_prompt_dialog(&mut self, frame: &mut Frame, area: Rect) {
        let Some(prompt) = self.state.interaction_prompt.as_ref() else {
            self.state.render_state.interaction_prompt_list_area = None;
            self.state.render_state.interaction_prompt_supplement_area = None;
            return;
        };

        let available_width = area.width.saturating_sub(4).max(1);
        let width = if available_width >= 36 {
            available_width.min(88).max(36)
        } else {
            available_width
        };
        let inner_width = width.saturating_sub(2);
        let max_height = (area.height as f32 * 0.8).ceil() as u16;
        let available_height = area.height.saturating_sub(4).max(1).min(max_height);
        let desired_height = interaction_prompt_outer_height(&prompt.request, inner_width).max(6);
        let height = desired_height.min(available_height);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let dialog_area = Rect {
            x,
            y,
            width,
            height,
        };

        render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
        render_interaction_prompt(
            frame,
            dialog_area,
            prompt,
            &self.state.theme,
            &mut self.state.render_state.interaction_prompt_list_area,
            &mut self.state.render_state.interaction_prompt_supplement_area,
        );
    }

    pub(crate) fn render_slash_popup_dialog(&mut self, frame: &mut Frame, area: Rect) {
        if !self.state.slash_menu_visible() {
            self.state.render_state.slash_popup_inner = None;
            return;
        }

        let value = self.state.chat_state.input.value();
        let cursor = self.state.chat_state.input.cursor();
        let candidates: Vec<String> = crate::slash_complete::slash_typed_prefix(&value, cursor)
            .map(|prefix| {
                crate::slash_complete::candidates_for_prefix(&prefix, &self.state.external_commands)
            })
            .unwrap_or_default();
        if candidates.is_empty() {
            self.state.render_state.slash_popup_inner = None;
            return;
        }

        let available_width = area.width.saturating_sub(4).max(1);
        let width = if available_width >= 32 {
            available_width.min(64).max(32)
        } else {
            available_width
        };
        let available_height = area.height.saturating_sub(4).max(1);
        let desired_height = (candidates.len() as u16 + 2).max(4);
        let height = desired_height.min(available_height);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let dialog_area = Rect {
            x,
            y,
            width,
            height,
        };

        render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
        let refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
        self.render_slash_popup(frame, dialog_area, &refs, self.state.slash.selected);
    }

    pub(crate) fn render_slash_popup(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        candidates: &[&str],
        selected: usize,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border_active))
            .title(" / 命令 ")
            .style(Style::default().bg(self.state.theme.background));
        let inner = block.inner(area);
        self.state.render_state.slash_popup_inner = Some(inner);
        let max_command_width = candidates
            .iter()
            .map(|command| command.chars().count())
            .max()
            .unwrap_or(0);
        let items: Vec<ListItem> = candidates
            .iter()
            .enumerate()
            .map(|(index, command)| {
                let is_selected = index == selected;
                let style = if is_selected {
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .bg(self.state.theme.selection)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.state.theme.foreground)
                };
                let mut spans = vec![Span::styled(
                    format!("{command:<width$}", width = max_command_width),
                    style,
                )];
                if let Some(summary) = crate::slash_complete::summary_for_command(
                    command,
                    &self.state.external_commands,
                ) {
                    spans.push(Span::styled(
                        format!("  {}", summary),
                        Style::default().fg(self.state.theme.muted),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    pub(crate) fn render_input(&self, frame: &mut Frame, area: Rect) {
        let has_tool_cards = self.state.active_transcript_has_tool_cards();
        let readonly_subagent_view = self.state.is_subagent_view_active()
            && self.state.input_mode == InputMode::Editing
            && self.state.api_key_dialog.is_none()
            && self.state.provider_dialog.is_none()
            && self.state.sandbox_dialog.is_none()
            && self.state.remote_session_dialog.is_none()
            && self.state.session_snapshot_dialog.is_none()
            && self.state.delete_dialog.is_none()
            && self.state.interaction_prompt.is_none();
        let title = if self.state.input_mode == InputMode::InteractionPrompt {
            " ↑↓ 选择 | Enter 确认 | Esc 取消 | Tab 切换补充 "
        } else if self.state.slash_menu_visible() {
            " ↑↓ 选择 | Enter 补全 | Esc 关闭列表 | Ctrl+C 退出 "
        } else if self.state.api_key_dialog.is_some() {
            " Enter 连接 | Esc 取消 "
        } else if self.state.session_snapshot_dialog.is_some() {
            " ↑↓ 选择快照 | Enter 读取 | Esc 取消 "
        } else if self.state.remote_session_dialog.is_some() {
            " ↑↓ 选择 remote session | Enter 确认 | Esc 取消 "
        } else if self.state.delete_dialog.is_some() {
            " ↑↓ 选择 | Enter 确认 | Esc 取消 "
        } else if self.state.provider_dialog.is_some() {
            " ↑↓ 切换 | ←→ 分栏 | Enter 选择 | Esc 关闭 "
        } else if readonly_subagent_view {
            " Subagent view is read-only | Shift+↑ Back "
        } else if self.state.chat_state.is_loading {
            " Enter 加入队列 | Esc 取消当前任务 "
        } else if has_tool_cards {
            " Enter 发送 | Ctrl+J 换行 | / 命令 | Click 工具详情 | Ctrl+C 退出 "
        } else {
            " Enter 发送 | Ctrl+J 换行 | / 命令 | Ctrl+C 退出 "
        };
        let input_style = self.state.theme.default_style();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.state.theme.border_style(true))
            .title(sanitize_terminal_text(title))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.state.theme.input_bg));

        let inner = block.inner(area);
        let readonly_value;
        let (value, cursor, selection) = if readonly_subagent_view {
            readonly_value = self.state.active_subagent_readonly_text();
            (
                readonly_value.as_str(),
                readonly_value.chars().count(),
                None,
            )
        } else {
            (
                self.state.chat_state.input.value(),
                self.state.chat_state.input.cursor(),
                self.state.chat_state.input.selected_range(),
            )
        };

        let inner_height = inner.height.max(1) as usize;
        let max_width = inner.width.max(1) as usize;

        // Calculate visual cursor position considering line wrapping
        let (visual_row, visual_col) = calculate_visual_cursor_position(value, cursor, max_width);
        let scroll_y = visual_row.saturating_sub(inner_height.saturating_sub(1));

        let selection_style = Style::default()
            .fg(self.state.theme.background)
            .bg(self.state.theme.foreground)
            .add_modifier(Modifier::BOLD);

        let paragraph = if let Some(sel_range) = selection {
            // Build a Text with selection highlighting.
            let text =
                build_input_text_with_selection(value, &sel_range, input_style, selection_style);
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll_y as u16, 0))
                .block(block)
        } else {
            Paragraph::new(value)
                .style(input_style)
                .wrap(Wrap { trim: false })
                .scroll((scroll_y as u16, 0))
                .block(block)
        };
        frame.render_widget(paragraph, area);

        if inner.width > 0
            && inner.height > 0
            && self.state.interaction_prompt.is_none()
            && self.state.api_key_dialog.is_none()
            && self.state.provider_dialog.is_none()
            && self.state.sandbox_dialog.is_none()
            && self.state.remote_session_dialog.is_none()
            && self.state.session_snapshot_dialog.is_none()
            && !readonly_subagent_view
            && matches!(
                self.state.input_mode,
                InputMode::Editing
                    | InputMode::ProviderSelection
                    | InputMode::SandboxSelection
                    | InputMode::RemoteSessionSelection
                    | InputMode::SessionSnapshotSelection
            )
        {
            let y_on_screen = visual_row.saturating_sub(scroll_y);
            if y_on_screen < inner_height {
                let adjusted_visual_col = if visual_col >= inner.width as usize {
                    inner.width.saturating_sub(1) as usize
                } else {
                    visual_col
                };
                let cursor_x = inner.x.saturating_add(adjusted_visual_col as u16);
                let cursor_y = inner.y.saturating_add(y_on_screen as u16);
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }
    }

    pub(crate) fn render_provider_dialog(
        &self,
        frame: &mut Frame,
        area: Rect,
        dialog: &ProviderDialog,
    ) {
        let dialog_width = area.width.min(80).max(60);
        let dialog_height = area.height.min(20).max(12);
        let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect {
            x: dialog_x,
            y: dialog_y,
            width: dialog_width,
            height: dialog_height,
        };

        render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
        let dialog_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border_active))
            .style(Style::default().bg(self.state.theme.background));
        let inner = dialog_block.inner(dialog_area);
        frame.render_widget(dialog_block, dialog_area);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);
        let left = horizontal[0];
        let right = horizontal[1];

        let providers: Vec<Line> = dialog
            .providers
            .iter()
            .enumerate()
            .map(|(index, provider)| {
                let style = if index == dialog.selected_provider {
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .bg(self.state.theme.selection)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.state.theme.foreground)
                };
                Line::from(Span::styled(provider.name.clone(), style))
            })
            .collect();

        let provider_list = List::new(providers).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(self.state.theme.border_style(true))
                .title(" Providers ")
                .padding(Padding::horizontal(1)),
        );
        frame.render_widget(provider_list, left);

        let models = dialog.current_models();
        let model_lines: Vec<Line> = if dialog.local_models_loading {
            vec![Line::from(Span::styled(
                "Loading models...",
                Style::default()
                    .fg(self.state.theme.foreground)
                    .add_modifier(Modifier::ITALIC),
            ))]
        } else {
            models
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    let style = if index == dialog.selected_model {
                        Style::default()
                            .fg(self.state.theme.foreground)
                            .bg(self.state.theme.selection)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(self.state.theme.foreground)
                    };
                    Line::from(Span::styled(model.name.clone(), style))
                })
                .collect()
        };

        let model_list = List::new(model_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(self.state.theme.border_style(true))
                .title(" Models ")
                .padding(Padding::horizontal(1)),
        );
        frame.render_widget(model_list, right);
    }

    pub(crate) fn render_sandbox_dialog(
        &self,
        frame: &mut Frame,
        area: Rect,
        dialog: &SandboxDialog,
    ) {
        let dialog_width = area.width.min(72).max(48);
        let visible = dialog.options.len() as u16;
        let dialog_height = area.height.min(visible + 4).max(7);
        let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect {
            x: dialog_x,
            y: dialog_y,
            width: dialog_width,
            height: dialog_height,
        };

        render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
        let block = Block::default()
            .title(" Sandbox ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border_active))
            .style(Style::default().bg(self.state.theme.background))
            .padding(Padding::horizontal(1));
        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        let list_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };

        let name_width = dialog
            .options
            .iter()
            .map(|option| option.name.chars().count())
            .max()
            .unwrap_or(0);
        let items: Vec<ListItem> = dialog
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let style = if index == dialog.selected {
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .bg(self.state.theme.selection)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.state.theme.foreground)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<name_width$}", option.name), style),
                    Span::styled("  ", style),
                    Span::styled(option.description, style),
                ]))
            })
            .collect();

        let mut list_state = ListState::default();
        list_state.select(Some(dialog.selected));
        frame.render_stateful_widget(List::new(items), list_area, &mut list_state);

        let hint = Paragraph::new("↑↓ 选择  Enter 应用  Esc 取消")
            .style(Style::default().fg(self.state.theme.muted));
        frame.render_widget(hint, hint_area);
    }

    pub(crate) fn render_remote_session_dialog(
        &self,
        frame: &mut Frame,
        area: Rect,
        dialog: &RemoteSessionDialog,
    ) {
        let dialog_width = area.width.min(92).max(56);
        let desired_height = match dialog.mode {
            RemoteSessionDialogMode::List => (dialog.entries.len() as u16 + 4).clamp(8, 22),
            RemoteSessionDialogMode::NewUrl => 9,
        };
        let dialog_height = area.height.min(desired_height).max(8);
        let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect {
            x: dialog_x,
            y: dialog_y,
            width: dialog_width,
            height: dialog_height,
        };

        render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
        let title = if dialog.switch_only {
            " Switch Session "
        } else {
            " Remote Sessions "
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.state.theme.border_style(true))
            .style(Style::default().bg(self.state.theme.background))
            .padding(Padding::horizontal(1));
        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        match dialog.mode {
            RemoteSessionDialogMode::List => {
                let list_area = Rect {
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: inner.height.saturating_sub(1),
                };
                let hint_area = Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width,
                    height: 1,
                };
                // In switch mode every row belongs to the same daemon, so
                // drop the daemon column and show a session_id tail instead.
                // Reserve 2 cols for the current-session marker (`* ` / `  `).
                let marker_width = 2usize;
                let time_width = 12usize;
                let daemon_width = if dialog.switch_only {
                    0usize
                } else {
                    (list_area.width / 3).clamp(16, 28) as usize
                };
                let id_width = if dialog.switch_only { 14usize } else { 0usize };
                let preview_width = list_area
                    .width
                    .saturating_sub(daemon_width as u16)
                    .saturating_sub(id_width as u16)
                    .saturating_sub(time_width as u16)
                    .saturating_sub(marker_width as u16)
                    .saturating_sub(6) as usize;
                let items: Vec<ListItem> = dialog
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        let selected = index == dialog.selected;
                        let style = if selected {
                            Style::default()
                                .fg(self.state.theme.foreground)
                                .bg(self.state.theme.selection)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(self.state.theme.foreground)
                        };
                        match entry {
                            RemoteSessionDialogEntry::Existing(record) => {
                                let is_current = dialog
                                    .current_session_id
                                    .as_deref()
                                    .map(|id| id == record.session_id.as_str())
                                    .unwrap_or(false);
                                let marker = if is_current { "* " } else { "  " };
                                let marker_style = if is_current {
                                    Style::default().fg(self.state.theme.accent)
                                } else {
                                    style
                                };
                                let mut spans = Vec::with_capacity(
                                    1 + 2 * ((daemon_width > 0) as usize)
                                        + 2 * ((id_width > 0) as usize)
                                        + 3,
                                );
                                spans.push(Span::styled(marker, marker_style));
                                if daemon_width > 0 {
                                    let daemon = truncate_chars(
                                        &daemon_display(&record.base_url),
                                        daemon_width,
                                    );
                                    spans.push(Span::styled(
                                        format!("{daemon:<daemon_width$}"),
                                        style,
                                    ));
                                    spans.push(Span::styled("  ", style));
                                }
                                if id_width > 0 {
                                    let id_tail = truncate_chars(&record.session_id, id_width);
                                    spans
                                        .push(Span::styled(format!("{id_tail:<id_width$}"), style));
                                    spans.push(Span::styled("  ", style));
                                }
                                let time = format_remote_time(record.last_active_at_ms);
                                spans.push(Span::styled(format!("{time:<time_width$}"), style));
                                spans.push(Span::styled("  ", style));
                                let preview = record
                                    .first_message_preview
                                    .as_deref()
                                    .filter(|value| !value.trim().is_empty())
                                    .unwrap_or("No messages yet");
                                let preview = truncate_chars(preview, preview_width.max(8));
                                spans.push(Span::styled(preview, style));
                                ListItem::new(Line::from(spans))
                            }
                            RemoteSessionDialogEntry::New => ListItem::new(Line::from(vec![
                                Span::styled("New remote session...", style),
                                Span::styled(
                                    "  configure daemon URL",
                                    Style::default().fg(self.state.theme.muted),
                                ),
                            ])),
                        }
                    })
                    .collect();
                let mut list_state = ListState::default();
                list_state.select(Some(dialog.selected));
                frame.render_stateful_widget(List::new(items), list_area, &mut list_state);

                let hint_text = if dialog.switch_only {
                    "* 当前 session  Enter 切换  Esc 取消"
                } else {
                    "Enter 继续/新建  Esc 取消"
                };
                let hint =
                    Paragraph::new(hint_text).style(Style::default().fg(self.state.theme.muted));
                frame.render_widget(hint, hint_area);
            }
            RemoteSessionDialogMode::NewUrl => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .split(inner);
                frame.render_widget(
                    Paragraph::new("Daemon URL / IP:port")
                        .style(Style::default().fg(self.state.theme.muted)),
                    chunks[0],
                );
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(self.state.theme.border_style(true))
                    .padding(Padding::horizontal(1));
                let input_inner = input_block.inner(chunks[1]);
                frame.render_widget(
                    Paragraph::new(dialog.url_input.value())
                        .style(self.state.theme.default_style())
                        .block(input_block),
                    chunks[1],
                );
                let cursor_x = input_inner.x.saturating_add(
                    line_prefix_width(dialog.url_input.value(), dialog.url_input.cursor())
                        .min(input_inner.width.saturating_sub(1) as usize)
                        as u16,
                );
                frame.set_cursor_position((cursor_x, input_inner.y));

                if let Some(error) = dialog.error.as_ref() {
                    frame.render_widget(
                        Paragraph::new(error.as_str())
                            .style(Style::default().fg(self.state.theme.error)),
                        chunks[2],
                    );
                }
                frame.render_widget(
                    Paragraph::new("Enter 连接并创建新的本地 session  Esc 返回列表")
                        .style(Style::default().fg(self.state.theme.muted))
                        .wrap(Wrap { trim: true }),
                    chunks[3],
                );
            }
        }
    }

    pub(crate) fn render_session_snapshot_dialog(
        &self,
        frame: &mut Frame,
        area: Rect,
        dialog: &SessionSnapshotDialog,
    ) {
        let dialog_width = area.width.min(86).max(54);
        let desired_height = (dialog.entries.len() as u16 + 4).clamp(8, 22);
        let dialog_height = area.height.min(desired_height).max(8);
        let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect {
            x: dialog_x,
            y: dialog_y,
            width: dialog_width,
            height: dialog_height,
        };

        render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
        let block = Block::default()
            .title(" Load Session ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.state.theme.border_style(true))
            .style(Style::default().bg(self.state.theme.background))
            .padding(Padding::horizontal(1));
        let inner = block.inner(dialog_area);

        let name_width = dialog
            .entries
            .iter()
            .map(|entry| entry.name.chars().count())
            .max()
            .unwrap_or(4)
            .clamp(8, 28);
        let items: Vec<ListItem> = dialog
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let selected = index == dialog.selected;
                let style = if selected {
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .bg(self.state.theme.selection)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.state.theme.foreground)
                };
                let prefix = if entry.depth == 0 {
                    String::new()
                } else {
                    format!("{}└ ", "  ".repeat(entry.depth.saturating_sub(1)))
                };
                let name = truncate_chars(&format!("{prefix}{}", entry.name), name_width);
                let time = format_snapshot_time(entry.saved_at_ms);
                let mut spans = vec![
                    Span::styled(format!("{name:<name_width$}"), style),
                    Span::styled("  ", style),
                    Span::styled(time, style),
                ];
                if let Some(parent) = entry.parent_name.as_ref() {
                    spans.push(Span::styled(
                        "  fork: ",
                        Style::default().fg(self.state.theme.muted),
                    ));
                    spans.push(Span::styled(
                        parent.clone(),
                        Style::default().fg(self.state.theme.muted),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items).block(block);
        frame.render_widget(list, dialog_area);

        let hint = Paragraph::new("Enter 读取  Esc 取消")
            .style(Style::default().fg(self.state.theme.muted))
            .wrap(Wrap { trim: true });
        let hint_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(hint, hint_area);
    }

    pub(crate) fn render_delete_dialog(
        &self,
        frame: &mut Frame,
        area: Rect,
        dialog: &DeleteDialog,
    ) {
        match dialog {
            DeleteDialog::Selecting { entries, selected } => {
                let dialog_width = area.width.min(80).max(50);
                let max_visible = 8u16;
                let visible = (entries.len() as u16).min(max_visible);
                let desired_height = visible + 3;
                let dialog_height = area.height.min(desired_height).max(6);
                let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
                let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
                let dialog_area = Rect {
                    x: dialog_x,
                    y: dialog_y,
                    width: dialog_width,
                    height: dialog_height,
                };

                render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
                let block = Block::default()
                    .title(" Select a turn to delete ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(self.state.theme.border_style(true))
                    .style(Style::default().bg(self.state.theme.background))
                    .padding(Padding::horizontal(1));
                let inner = block.inner(dialog_area);

                frame.render_widget(block, dialog_area);

                // Separate list and hint areas to avoid overlap
                let list_area = Rect {
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: inner.height.saturating_sub(1),
                };
                let hint_area = Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width,
                    height: 1,
                };

                let prefix_width = format!("{}. ", entries.len()).len() as u16;
                let content_width = list_area.width.saturating_sub(prefix_width);

                let items: Vec<ListItem> = entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        let is_selected = index == *selected;
                        let style = if is_selected {
                            Style::default()
                                .fg(self.state.theme.foreground)
                                .bg(self.state.theme.selection)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(self.state.theme.foreground)
                        };
                        let display = truncate_to_width(&entry.full_content, content_width);
                        ListItem::new(Line::from(Span::styled(
                            format!("{}. {}", entry.index, display),
                            style,
                        )))
                    })
                    .collect();

                // Use ListState for automatic scrolling
                let mut list_state = ListState::default();
                list_state.select(Some(*selected));
                let list = List::new(items);
                frame.render_stateful_widget(list, list_area, &mut list_state);

                let hint = Paragraph::new("↑↓ 选择  Enter 确认  Esc 取消")
                    .style(Style::default().fg(self.state.theme.muted));
                frame.render_widget(hint, hint_area);
            }
            DeleteDialog::Confirming {
                turn,
                subsequent_count,
            } => {
                let dialog_width = area.width.min(50).max(36);
                let dialog_height = if *subsequent_count > 0 { 8 } else { 6 };
                let dialog_x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
                let dialog_y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
                let dialog_area = Rect {
                    x: dialog_x,
                    y: dialog_y,
                    width: dialog_width,
                    height: dialog_height,
                };

                render_popup_backdrop(frame, dialog_area, area, self.state.theme.background);
                let block = Block::default()
                    .title(" 确认删除 ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(self.state.theme.border_active))
                    .style(Style::default().bg(self.state.theme.background))
                    .padding(Padding::horizontal(1));
                let _inner = block.inner(dialog_area);
                let label = "删除对话: ";
                let content_width = _inner.width.saturating_sub(label.len() as u16);
                let display = truncate_to_width(&turn.full_content, content_width);

                let mut lines: Vec<Line<'_>> = vec![
                    Line::from(Span::styled(
                        format!("{}{}", label, display),
                        Style::default().fg(self.state.theme.foreground),
                    )),
                    Line::from(""),
                ];

                if *subsequent_count > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("⚠ 该轮次之后还有 {} 轮对话，", subsequent_count),
                        Style::default().fg(self.state.theme.error),
                    )));
                    lines.push(Line::from(Span::styled(
                        "删除后可能丢失相关上下文。".to_string(),
                        Style::default().fg(self.state.theme.error),
                    )));
                }

                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "Enter 确认",
                        Style::default()
                            .fg(self.state.theme.foreground)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled("Esc 取消", Style::default().fg(self.state.theme.muted)),
                ]));

                let paragraph = Paragraph::new(lines).block(block);
                frame.render_widget(paragraph, dialog_area);
            }
        }
    }

    pub(crate) fn render_api_key_dialog(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        dialog: &ApiKeyDialogState,
    ) {
        let width = area.width.min(56).max(40);
        let height = 8u16;
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let rect = Rect {
            x,
            y,
            width,
            height,
        };

        render_popup_backdrop(frame, rect, area, self.state.theme.background);
        let title = sanitize_terminal_text(&format!(
            " Enter API key — {} / {} ",
            dialog.provider, dialog.model
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border_active))
            .title(title);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(inner);

        let hint = Paragraph::new("Enter your API key, then press Enter. Esc to cancel.")
            .style(Style::default().fg(self.state.theme.muted));
        frame.render_widget(hint, chunks[0]);

        let toggle_label = if dialog.show_plaintext {
            "Hide"
        } else {
            "Show"
        };
        let toggle_width = toggle_label.chars().count() as u16;
        let row_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(toggle_width.saturating_add(2)),
            ])
            .split(chunks[1]);

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .padding(Padding::horizontal(1));
        let input_inner = input_block.inner(row_chunks[0]);
        frame.render_widget(
            input_block.style(Style::default().bg(self.state.theme.input_bg)),
            row_chunks[0],
        );

        let display_value = if dialog.show_plaintext {
            dialog.input.value().to_string()
        } else {
            "*".repeat(dialog.input.value().chars().count())
        };
        let input_paragraph = Paragraph::new(display_value).style(
            Style::default()
                .fg(self.state.theme.foreground)
                .bg(self.state.theme.input_bg),
        );
        frame.render_widget(input_paragraph, input_inner);

        let toggle_area = Rect {
            x: row_chunks[1].x,
            y: row_chunks[1].y + 1,
            width: row_chunks[1].width,
            height: 1,
        };
        let toggle_button =
            Paragraph::new(toggle_label).style(Style::default().fg(self.state.theme.primary));
        frame.render_widget(toggle_button, toggle_area);
        self.state.render_state.api_key_toggle_area = Some(toggle_area);

        let cursor_offset = if dialog.show_plaintext {
            dialog.input.visual_cursor() as u16
        } else {
            dialog.input.cursor() as u16
        };
        let cursor_x = (input_inner.x + cursor_offset)
            .min(input_inner.x + input_inner.width.saturating_sub(1));
        frame.set_cursor_position((cursor_x, input_inner.y));

        if let Some(error) = &dialog.error {
            let error_paragraph = Paragraph::new(error.as_str())
                .style(self.state.theme.error_style())
                .wrap(Wrap { trim: true });
            frame.render_widget(error_paragraph, chunks[2]);
        }
    }
}

fn one_line_preview(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_chars == 0 {
        return String::new();
    }
    let mut chars = collapsed.chars();
    let preview: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

/// Build a ratatui `Text` value for the input box where the characters in
/// `sel_range` (char indices) are highlighted with `sel_style`.
fn build_input_text_with_selection(
    value: &str,
    sel_range: &std::ops::Range<usize>,
    normal_style: Style,
    sel_style: Style,
) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut char_offset: usize = 0;

    for source_line in value.split('\n') {
        let line_len = source_line.chars().count();
        let line_end = char_offset + line_len;

        let overlap_start = sel_range.start.max(char_offset);
        let overlap_end = sel_range.end.min(line_end);

        let mut spans: Vec<Span<'static>> = Vec::new();
        if overlap_start >= overlap_end {
            // No selection overlap on this line.
            spans.push(Span::styled(source_line.to_owned(), normal_style));
        } else {
            // Before selection
            let before: String = source_line
                .chars()
                .take(overlap_start - char_offset)
                .collect();
            if !before.is_empty() {
                spans.push(Span::styled(before, normal_style));
            }
            // Selected portion
            let sel_local_start = overlap_start - char_offset;
            let sel_local_end = overlap_end - char_offset;
            let selected: String = source_line
                .chars()
                .skip(sel_local_start)
                .take(sel_local_end - sel_local_start)
                .collect();
            if !selected.is_empty() {
                spans.push(Span::styled(selected, sel_style));
            }
            // After selection
            let after: String = source_line.chars().skip(sel_local_end).collect();
            if !after.is_empty() {
                spans.push(Span::styled(after, normal_style));
            }
        }

        lines.push(Line::from(spans));
        // +1 accounts for the '\n' that was consumed by split.
        char_offset = line_end + 1;
    }

    Text::from(lines)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut truncated: String = value.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

/// Commit pending whitespace then pending word into the current line at
/// `(row, line_width)`, recording each char's visual position and advancing
/// the line width. Both pending buffers are drained and their width trackers
/// reset to 0. Returns the updated line width.
fn flush_pending(
    pos: &mut [(usize, usize)],
    chars: &[char],
    pending_ws: &mut Vec<usize>,
    pending_ws_width: &mut usize,
    pending_word: &mut Vec<usize>,
    pending_word_width: &mut usize,
    row: usize,
    line_width: usize,
) -> usize {
    let mut lw = line_width;
    for ci in pending_ws.drain(..) {
        pos[ci] = (row, lw);
        lw += char_display_width(chars[ci]);
    }
    *pending_ws_width = 0;
    for ci in pending_word.drain(..) {
        pos[ci] = (row, lw);
        lw += char_display_width(chars[ci]);
    }
    *pending_word_width = 0;
    lw
}

/// Calculate the visual cursor position considering automatic line wrapping.
///
/// Faithfully mirrors ratatui 0.28's `WordWrapper` with `Wrap { trim: false }`,
/// which is exactly what the input `Paragraph` uses. The previous
/// implementation trimmed trailing whitespace from each wrapped line, but
/// ratatui keeps it on the current line (filling up to the column width) and
/// only consumes whitespace that would overflow the break. That mismatch left
/// the cursor on the wrong row/column whenever long text wrapped around runs
/// of spaces, hit exact-fill boundaries, or mixed CJK with word breaks.
///
/// `pos[p]` holds the visual (row, col) where character `p` is rendered, and
/// `pos[len]` the end-of-text cursor. `pos[cursor]` is returned.
fn calculate_visual_cursor_position(
    value: &str,
    cursor: usize,
    max_width: usize,
) -> (usize, usize) {
    if max_width == 0 || value.is_empty() {
        return (0, 0);
    }

    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    let cursor = cursor.min(n);

    let mut pos: Vec<(usize, usize)> = vec![(0, 0); n + 1];

    // ratatui WordWrapper state (trim == false).
    let mut row = 0usize;
    let mut line_width = 0usize; // width of committed pending_line content
    let mut non_ws_prev = false;
    let mut pending_word: Vec<usize> = Vec::new();
    let mut pending_word_width = 0usize;
    let mut pending_ws: Vec<usize> = Vec::new();
    let mut pending_ws_width = 0usize;

    for (idx, &ch) in chars.iter().enumerate() {
        // ratatui splits Paragraph text on '\n' into separate input Lines
        // before wrapping, so a newline is a hard line break.
        if ch == '\n' {
            line_width = flush_pending(
                &mut pos,
                &chars,
                &mut pending_ws,
                &mut pending_ws_width,
                &mut pending_word,
                &mut pending_word_width,
                row,
                line_width,
            );
            pos[idx] = (row, line_width);
            row += 1;
            line_width = 0;
            non_ws_prev = false;
            continue;
        }

        let sw = char_display_width(ch);
        if sw > max_width {
            // ratatui silently drops symbols wider than the line limit.
            pos[idx] = (row, line_width);
            non_ws_prev = !ch.is_whitespace();
            continue;
        }
        let is_ws = ch.is_whitespace();

        let word_found = non_ws_prev && is_ws;
        // `untrimmed_overflow` only fires when trim == false (our case) and the
        // line has no committed content yet but the symbol would overflow.
        let untrimmed_overflow =
            line_width == 0 && pending_word_width + pending_ws_width + sw > max_width;

        if word_found || untrimmed_overflow {
            // commit pending whitespace then pending word into the current line
            line_width = flush_pending(
                &mut pos,
                &chars,
                &mut pending_ws,
                &mut pending_ws_width,
                &mut pending_word,
                &mut pending_word_width,
                row,
                line_width,
            );
        }

        let line_full = line_width >= max_width;
        let pending_word_overflow =
            sw > 0 && line_width + pending_ws_width + pending_word_width >= max_width;

        if line_full || pending_word_overflow {
            let pushed_row = row;
            let pushed_width = line_width;
            line_width = 0;
            row += 1;

            // ratatui consumes leading pending whitespace to fill the remainder
            // of the just-pushed line (rendered as background). Those chars land
            // on the pushed line, not the next one.
            let mut remaining = max_width.saturating_sub(pushed_width);
            let mut fill_col = pushed_width;
            while let Some(&ci) = pending_ws.first() {
                let cw = char_display_width(chars[ci]);
                if cw > remaining {
                    break;
                }
                pos[ci] = (pushed_row, fill_col);
                fill_col += cw;
                remaining -= cw;
                pending_ws.remove(0);
                pending_ws_width -= cw;
            }

            // When the pending whitespace buffer is fully consumed at the break,
            // ratatui drops the current whitespace symbol (the first one of the
            // next word). Position it at the end of the just-pushed line so the
            // cursor clamps there instead of jumping to the next row prematurely.
            if is_ws && pending_ws.is_empty() {
                pos[idx] = (pushed_row, max_width);
                non_ws_prev = false;
                continue;
            }
        }

        if is_ws {
            pending_ws.push(idx);
            pending_ws_width += sw;
        } else {
            pending_word.push(idx);
            pending_word_width += sw;
        }
        non_ws_prev = !is_ws;
    }

    // end-of-input tail, mirroring ratatui's `process_input`
    if line_width == 0 && pending_word.is_empty() && !pending_ws.is_empty() {
        // trailing whitespace with no following word on an empty line: ratatui
        // emits a blank line first, then the whitespace starts the next row.
        row += 1;
    }
    line_width = flush_pending(
        &mut pos,
        &chars,
        &mut pending_ws,
        &mut pending_ws_width,
        &mut pending_word,
        &mut pending_word_width,
        row,
        line_width,
    );
    pos[n] = (row, line_width);

    pos[cursor]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::{Paragraph, Widget, Wrap};

    #[test]
    fn test_basic() {
        let (row, col) = calculate_visual_cursor_position("hello", 3, 10);
        assert_eq!((row, col), (0, 3));
    }

    #[test]
    fn test_newline() {
        let (row, col) = calculate_visual_cursor_position("hello\nworld", 6, 10);
        assert_eq!((row, col), (1, 0));
    }

    #[test]
    fn test_cursor_after_newline() {
        let (row, col) = calculate_visual_cursor_position("hello\n", 6, 10);
        assert_eq!((row, col), (1, 0));
    }

    #[test]
    fn test_empty() {
        let (row, col) = calculate_visual_cursor_position("", 0, 10);
        assert_eq!((row, col), (0, 0));
    }

    #[test]
    fn test_multiple_spaces_wrap() {
        // "aa   bb" at width 4: ratatui renders "aa  " / "bb  " (two trailing
        // spaces fill the first line as background). Cursor at the end (7) must
        // be on row 1 at the end of "bb" (col 2).
        assert_eq!(calculate_visual_cursor_position("aa   bb", 7, 4), (1, 2));
        // cursor before the first 'b' lands at the start of row 1.
        assert_eq!(calculate_visual_cursor_position("aa   bb", 5, 4), (1, 0));
    }

    #[test]
    fn test_long_word_break() {
        // "aaaaaaaa" at width 4 wraps as "aaaa" / "aaaa"; cursor=4 is the start
        // of the second line, not the (off-screen) end of the first.
        assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 4, 4), (1, 0));
        assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 8, 4), (1, 4));
    }

    #[test]
    fn test_cjk_wrap() {
        // Each CJK char is width 2; width 6 fits three per line.
        assert_eq!(calculate_visual_cursor_position("中文测试", 3, 6), (1, 0));
        assert_eq!(calculate_visual_cursor_position("中文测试", 4, 6), (1, 2));
    }

    /// Differential test: for every cursor offset, the (row, col) returned
    /// must match the cell where ratatui's `Paragraph` + `Wrap { trim: false }`
    /// actually renders that character (off-screen columns are skipped, since
    /// the renderer clamps them separately).
    fn ratatui_cell(value: &str, width: u16, row: usize, col: usize) -> String {
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height: 80,
        };
        let mut buf = Buffer::empty(area);
        Widget::render(
            Paragraph::new(value).wrap(Wrap { trim: false }),
            area,
            &mut buf,
        );
        buf[(col as u16, row as u16)].symbol().to_string()
    }

    fn assert_matches_ratatui(value: &str, width: usize) {
        let chars: Vec<char> = value.chars().collect();
        for p in 0..=chars.len() {
            let (row, col) = calculate_visual_cursor_position(value, p, width);
            assert!(
                col <= width,
                "value={value:?} width={width} cursor={p}: col {col} > width {width}"
            );
            if p < chars.len() {
                let ch = chars[p];
                if ch == '\n' {
                    continue;
                }
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if w == 0 || col == width {
                    // zero-width chars and exact-fill boundary cursors have no
                    // unique on-screen cell to compare against.
                    continue;
                }
                let cell = ratatui_cell(value, width as u16, row, col);
                let cell_char = cell.chars().next();
                assert_eq!(
                    cell_char,
                    Some(ch),
                    "value={value:?} width={width} cursor={p} (char {ch:?}): \
                     calc=({row},{col}) but ratatui cell is {cell:?}"
                );
            }
        }
    }

    #[test]
    fn diff_basic_word() {
        assert_matches_ratatui("hello", 10);
    }

    #[test]
    fn diff_long_single_word_wraps_mid_word() {
        assert_matches_ratatui("aaaaaaaa", 4);
    }

    #[test]
    fn diff_word_wrap_with_spaces() {
        assert_matches_ratatui("aa bb cc dd", 4);
    }

    #[test]
    fn diff_multiple_spaces_boundary() {
        assert_matches_ratatui("aa   bb", 4);
    }

    #[test]
    fn diff_cursor_in_trailing_whitespace() {
        assert_matches_ratatui("aaaa ", 4);
    }

    #[test]
    fn diff_newline_then_wrap() {
        assert_matches_ratatui("hello\naaaaaaa", 4);
    }

    #[test]
    fn diff_cjk_wrap() {
        assert_matches_ratatui("中文测试一二三四五六", 6);
    }

    #[test]
    fn diff_spaces_then_newline() {
        assert_matches_ratatui("aa  \nbb", 4);
    }

    #[test]
    fn diff_long_paragraph() {
        assert_matches_ratatui(
            "The quick brown fox jumps over the lazy dog and keeps going",
            12,
        );
    }

    #[test]
    fn diff_consecutive_newlines() {
        assert_matches_ratatui("a\n\nb", 4);
    }
}
