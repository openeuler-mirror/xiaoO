use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::app_state::{ApiKeyDialogState, InputMode};
use crate::interaction_prompt::{interaction_prompt_outer_height, render_interaction_prompt};
use crate::provider_dialog::ProviderDialog;

use super::utils::{cursor_row_col, line_prefix_width};

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
            .title(title)
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.state.theme.input_bg));

        let inner = block.inner(area);
        let value = self.state.chat_state.input.value();
        let cursor = self.state.chat_state.input.cursor();
        let (row, col) = cursor_row_col(value, cursor);
        let lines: Vec<&str> = value.split('\n').collect();
        let line = lines.get(row).copied().unwrap_or("");

        let inner_height = inner.height.max(1) as usize;
        let scroll_y = row.saturating_sub(inner_height.saturating_sub(1));

        let max_width = inner.width.max(1).saturating_sub(1) as usize;
        let visual_x = line_prefix_width(line, col);
        let scroll_x = visual_x.max(max_width) - max_width;

        let paragraph = Paragraph::new(value)
            .style(input_style)
            .scroll((scroll_y as u16, scroll_x as u16))
            .block(block);
        frame.render_widget(paragraph, area);

        if !self.state.chat_state.is_loading
            && inner.width > 0
            && inner.height > 0
            && self.state.interaction_prompt.is_none()
            && self.state.api_key_dialog.is_none()
            && self.state.provider_dialog.is_none()
            && matches!(
                self.state.input_mode,
                InputMode::Editing | InputMode::ProviderSelection
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

    pub(crate) fn render_api_key_dialog(
        &self,
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

        let title = format!(" Enter API key — {} / {} ", dialog.provider, dialog.model);
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

        let mask = "*".repeat(dialog.input.value().len().min(40));
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .padding(Padding::horizontal(1));
        let input_paragraph = Paragraph::new(mask)
            .style(
                Style::default()
                    .fg(self.state.theme.foreground)
                    .bg(self.state.theme.input_bg),
            )
            .block(input_block);
        frame.render_widget(input_paragraph, chunks[1]);

        let cursor_x = (chunks[1].x + 1 + dialog.input.visual_cursor() as u16)
            .min(chunks[1].x + chunks[1].width.saturating_sub(2));
        frame.set_cursor_position((cursor_x, chunks[1].y + 1));

        if let Some(error) = &dialog.error {
            let error_paragraph = Paragraph::new(error.as_str())
                .style(self.state.theme.error_style())
                .wrap(Wrap { trim: true });
            frame.render_widget(error_paragraph, chunks[2]);
        }
    }
}
