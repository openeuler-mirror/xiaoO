use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::app_state::{ApiKeyDialogState, InputMode};
use crate::interaction_prompt::{interaction_prompt_outer_height, render_interaction_prompt};
use crate::provider_dialog::ProviderDialog;
use crate::session_snapshot_service::{format_snapshot_time, SessionSnapshotDialog};
use crate::services::turn_delete::DeleteDialog;

use super::utils::{cursor_row_col, line_prefix_width, sanitize_terminal_text};

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
        let available_height = area.height.saturating_sub(4).max(1);
        let desired_height = interaction_prompt_outer_height(&prompt.request).max(6);
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
        let has_tool_cards = self
            .state
            .chat_state
            .messages
            .iter()
            .any(|message| message.tool_state.is_some());
        let title = if self.state.chat_state.is_loading {
            " Esc 取消 "
        } else if self.state.input_mode == InputMode::InteractionPrompt {
            " ↑↓ 选择 | Enter 确认 | Esc 取消 | Tab 切换补充 "
        } else if self.state.slash_menu_visible() {
            " ↑↓ 选择 | Enter 补全 | Esc 关闭列表 | Ctrl+C 退出 "
        } else if self.state.api_key_dialog.is_some() {
            " Enter 连接 | Esc 取消 "
        } else if self.state.session_snapshot_dialog.is_some() {
            " ↑↓ 选择快照 | Enter 读取 | Esc 取消 "
        } else if self.state.delete_dialog.is_some() {
            " ↑↓ 选择 | Enter 确认 | Esc 取消 "
        } else if self.state.provider_dialog.is_some() {
            " ↑↓ 切换 | ←→ 分栏 | Enter 选择 | Esc 关闭 "
        } else if has_tool_cards {
            " Enter 发送 | / 命令 | Click 工具详情 | Ctrl+C 退出 "
        } else {
            " Enter 发送 | / 命令 | Ctrl+C 退出 "
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
        let value = self.state.chat_state.input.value();
        let cursor = self.state.chat_state.input.cursor();
        let selection = self.state.chat_state.input.selected_range();
        let (row, col) = cursor_row_col(value, cursor);
        let lines: Vec<&str> = value.split('\n').collect();
        let line = lines.get(row).copied().unwrap_or("");

        let inner_height = inner.height.max(1) as usize;
        let scroll_y = row.saturating_sub(inner_height.saturating_sub(1));

        let max_width = inner.width.max(1).saturating_sub(1) as usize;
        let visual_x = line_prefix_width(line, col);
        let scroll_x = visual_x.max(max_width) - max_width;

        let selection_style = Style::default()
            .fg(self.state.theme.background)
            .bg(self.state.theme.foreground)
            .add_modifier(Modifier::BOLD);

        let paragraph = if let Some(sel_range) = selection {
            // Build a Text with selection highlighting.
            let text =
                build_input_text_with_selection(value, &sel_range, input_style, selection_style);
            Paragraph::new(text)
                .scroll((scroll_y as u16, scroll_x as u16))
                .block(block)
        } else {
            Paragraph::new(value)
                .style(input_style)
                .scroll((scroll_y as u16, scroll_x as u16))
                .block(block)
        };
        frame.render_widget(paragraph, area);

        if !self.state.chat_state.is_loading
            && inner.width > 0
            && inner.height > 0
            && self.state.interaction_prompt.is_none()
            && self.state.api_key_dialog.is_none()
            && self.state.provider_dialog.is_none()
            && self.state.session_snapshot_dialog.is_none()
            && matches!(
                self.state.input_mode,
                InputMode::Editing
                    | InputMode::ProviderSelection
                    | InputMode::SessionSnapshotSelection
            )
        {
            let y_on_screen = row - scroll_y;
            if y_on_screen < inner_height {
                let x_on_screen = visual_x.saturating_sub(scroll_x);
                let cursor_x = inner.x.saturating_add(x_on_screen.min(max_width) as u16);
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
        let model_lines: Vec<Line> = models
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
            .collect();

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
                        format!(
                            "⚠ 该轮次之后还有 {} 轮对话，",
                            subsequent_count
                        ),
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
