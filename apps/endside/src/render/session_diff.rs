use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::chat::{TodoDisplayStatus, TodoMessageState};
use crate::render::utils::truncate_display_width;

impl App {
    pub(crate) fn render_sidebar(&self, frame: &mut Frame, area: Rect) {
        let Some(plan) = self.state.plan_state.as_ref() else {
            self.render_session_diff(frame, area);
            return;
        };

        if area.height < 12 {
            self.render_plan_panel(frame, area, plan);
            return;
        }

        let plan_height = plan_panel_height(plan, area.height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(plan_height), Constraint::Min(5)])
            .split(area);
        self.render_plan_panel(frame, chunks[0], plan);
        self.render_session_diff(frame, chunks[1]);
    }

    fn render_plan_panel(&self, frame: &mut Frame, area: Rect, plan: &TodoMessageState) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .title(" Plan ")
            .style(Style::default().bg(self.state.theme.background));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let completed = plan
            .items
            .iter()
            .filter(|(status, _)| *status == TodoDisplayStatus::Completed)
            .count();
        let total = plan.items.len();
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{completed}/{total} done"),
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    truncate_display_width(&plan.title, inner.width.saturating_sub(10) as usize),
                    Style::default().fg(self.state.theme.muted),
                ),
            ]),
            Line::raw(""),
        ];

        let max_lines = inner.height as usize;
        let mut shown_items = 0usize;
        for (status, content) in &plan.items {
            if lines.len() + 1 > max_lines {
                break;
            }
            shown_items += 1;
            let (marker, color) = match status {
                TodoDisplayStatus::Completed => ("x", self.state.theme.success),
                TodoDisplayStatus::InProgress => (">", self.state.theme.accent),
                TodoDisplayStatus::Pending => (" ", self.state.theme.muted),
            };
            let prefix = format!("[{marker}] ");
            let content_width = inner.width.saturating_sub(prefix.chars().count() as u16) as usize;
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(color)),
                Span::styled(
                    truncate_display_width(content, content_width),
                    Style::default().fg(self.state.theme.foreground),
                ),
            ]));
        }

        let remaining = plan.items.len().saturating_sub(shown_items);
        if remaining > 0 && lines.len() < max_lines {
            lines.push(Line::from(Span::styled(
                format!("… {remaining} more"),
                Style::default().fg(self.state.theme.muted),
            )));
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }

    pub(crate) fn render_session_diff(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .title(" Session Diff ")
            .style(Style::default().bg(self.state.theme.background));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let entries = self.state.sorted_session_file_changes();
        if entries.is_empty() {
            let empty = Paragraph::new("No file changes in this session yet.")
                .style(Style::default().fg(self.state.theme.muted));
            frame.render_widget(empty, inner);
            return;
        }

        let total_additions: u32 = entries.iter().map(|entry| entry.additions).sum();
        let total_deletions: u32 = entries.iter().map(|entry| entry.deletions).sum();
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{} files", entries.len()),
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("+{total_additions}"),
                    Style::default().fg(self.state.theme.success),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{total_deletions}"),
                    Style::default().fg(self.state.theme.error),
                ),
            ]),
            Line::raw(""),
        ];

        let max_lines = inner.height as usize;
        let mut shown_entries = 0usize;
        for entry in &entries {
            if lines.len() + 4 > max_lines {
                break;
            }
            shown_entries += 1;
            let display_path = self.state.display_file_path(&entry.file_path);
            lines.push(Line::from(Span::styled(
                truncate_display_width(&display_path, inner.width as usize),
                Style::default()
                    .fg(self.state.theme.foreground)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("+{}", entry.additions),
                    Style::default().fg(self.state.theme.success),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", entry.deletions),
                    Style::default().fg(self.state.theme.error),
                ),
            ]));
            lines.push(Line::raw(""));
        }

        let remaining = entries.len().saturating_sub(shown_entries);
        if remaining > 0 && lines.len() < max_lines {
            lines.push(Line::from(Span::styled(
                format!("… {remaining} more"),
                Style::default().fg(self.state.theme.muted),
            )));
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

fn plan_panel_height(plan: &TodoMessageState, available_height: u16) -> u16 {
    if available_height < 14 {
        return available_height;
    }
    let desired = (plan.items.len() as u16).saturating_add(4);
    desired.clamp(7, (available_height / 2).max(7))
}
