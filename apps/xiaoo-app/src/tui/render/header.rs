use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::status_panel::StatusPanel;

impl App {
    pub(crate) fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .style(Style::default().bg(self.state.theme.background));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let inner_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(14),
                Constraint::Min(1),
                Constraint::Length(10),
            ])
            .split(inner);

        let title = Paragraph::new(Line::from(vec![Span::styled(
            " XiaoO",
            Style::default()
                .fg(self.state.theme.accent)
                .add_modifier(Modifier::BOLD),
        )]));
        frame.render_widget(title, inner_chunks[0]);

        let tab = Paragraph::new(Line::from(vec![Span::styled(
            " Chat ",
            self.state.theme.tab_style(true),
        )]));
        frame.render_widget(tab, inner_chunks[1]);

        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        let status = Paragraph::new(Line::from(vec![Span::styled(
            now,
            Style::default().fg(self.state.theme.muted),
        )]))
        .alignment(Alignment::Right);
        frame.render_widget(status, inner_chunks[2]);
    }

    pub(crate) fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .title(Line::from(vec![Span::styled(
                " Status ",
                Style::default()
                    .fg(self.state.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )]))
            .style(self.state.theme.status_bar_style());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let provider_name = if self.state.status_panel.is_connected {
            format!(
                "{}/{}",
                self.state.status_panel.provider_name, self.state.status_panel.model_name
            )
        } else {
            "Disconnected".to_string()
        };
        let workspace = if self.state.status_panel.workspace_display.is_empty() {
            "—".to_string()
        } else {
            self.state.status_panel.workspace_display.clone()
        };
        let status_dot_style = if self.state.status_panel.is_connected {
            Style::default()
                .fg(self.state.theme.success)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.state.theme.error)
                .add_modifier(Modifier::BOLD)
        };
        let summary = Line::from(vec![
            Span::styled("● ", status_dot_style),
            Span::styled(
                provider_name,
                Style::default().fg(self.state.theme.foreground),
            ),
            Span::styled("  WS ", Style::default().fg(self.state.theme.muted)),
            Span::styled(workspace, Style::default().fg(self.state.theme.foreground)),
            Span::styled("  Tok ", Style::default().fg(self.state.theme.muted)),
            Span::styled(
                StatusPanel::format_token_count(self.state.status_panel.total_tokens),
                Style::default()
                    .fg(self.state.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(self.state.theme.muted)),
            Span::styled("(", Style::default().fg(self.state.theme.muted)),
            Span::styled("in ", Style::default().fg(self.state.theme.muted)),
            Span::styled(
                StatusPanel::format_token_count(self.state.status_panel.prompt_tokens),
                Style::default().fg(self.state.theme.foreground),
            ),
            Span::styled(" / ", Style::default().fg(self.state.theme.muted)),
            Span::styled("out ", Style::default().fg(self.state.theme.muted)),
            Span::styled(
                StatusPanel::format_token_count(self.state.status_panel.completion_tokens),
                Style::default().fg(self.state.theme.foreground),
            ),
            Span::styled(")  Ctx ", Style::default().fg(self.state.theme.muted)),
            Span::styled(
                StatusPanel::format_token_count(self.state.status_panel.context_tokens),
                Style::default()
                    .fg(self.state.theme.gradient_yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Lat ", Style::default().fg(self.state.theme.muted)),
            Span::styled(
                format!("{}ms", self.state.status_panel.last_latency_ms),
                Style::default()
                    .fg(self.state.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let summary_bar = Paragraph::new(summary);
        frame.render_widget(summary_bar, inner);
    }
}
