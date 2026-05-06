use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::app_state::RuntimeStatusLight;
use crate::status_panel::StatusPanel;

use super::utils::sanitize_terminal_text;

impl App {
    pub(crate) fn render_header(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .style(Style::default().bg(self.state.theme.background));
        let inner = block.inner(area);
        self.state.render_state.theme_toggle_area = None;
        frame.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let theme_button_text = format!(" {} ", self.state.theme.toggle_button_label());
        let theme_button_width = theme_button_text.chars().count() as u16;

        let inner_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(14),
                Constraint::Min(1),
                Constraint::Length(theme_button_width),
                Constraint::Length(32),
            ])
            .split(inner);

        let title = Paragraph::new(Line::from(vec![Span::styled(
            " XiaoO",
            Style::default()
                .fg(self.state.theme.accent)
                .add_modifier(Modifier::BOLD),
        )]));
        frame.render_widget(title, inner_chunks[0]);

        let mut tabs = Vec::new();
        for (index, label) in self.state.agent_tab_labels().iter().enumerate() {
            if index > 0 {
                tabs.push(Span::raw(" "));
            }
            tabs.push(Span::styled(
                format!(" {label} "),
                self.state
                    .theme
                    .tab_style(label == self.state.active_agent_tab_label()),
            ));
        }
        if let Some(role) = self.state.active_agent_role_config() {
            if !role.description.trim().is_empty() {
                tabs.push(Span::raw("  "));
                tabs.push(Span::styled(
                    role.description.as_str(),
                    Style::default().fg(self.state.theme.muted),
                ));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(tabs)), inner_chunks[1]);

        let theme_button_style = Style::default()
            .fg(self.state.theme.primary)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let theme_button = Paragraph::new(Line::from(vec![Span::styled(
            theme_button_text.clone(),
            theme_button_style,
        )]));
        frame.render_widget(theme_button, inner_chunks[2]);
        self.state.render_state.theme_toggle_area = Some(Rect {
            x: inner_chunks[2].x,
            y: inner_chunks[2].y,
            width: theme_button_width.min(inner_chunks[2].width),
            height: 1,
        });

        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        let (status_light_color, status_label, status_label_style) =
            match self.state.runtime_status_light() {
                RuntimeStatusLight::Running => (
                    self.state.theme.success,
                    "RUN",
                    Style::default()
                        .fg(self.state.theme.success)
                        .add_modifier(Modifier::BOLD),
                ),
                RuntimeStatusLight::AwaitingInteraction => (
                    self.state.theme.gradient_yellow,
                    "ASK",
                    Style::default()
                        .fg(self.state.theme.gradient_yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                RuntimeStatusLight::Idle => (
                    self.state.theme.foreground,
                    "IDLE",
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
            };
        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                sanitize_terminal_text("● "),
                Style::default().fg(status_light_color),
            ),
            Span::styled(format!("{status_label} "), status_label_style),
            Span::styled(
                format!("think:{} ", self.state.reasoning_effort),
                Style::default().fg(self.state.theme.primary),
            ),
            Span::styled(now, Style::default().fg(self.state.theme.muted)),
        ]))
        .alignment(Alignment::Right);
        frame.render_widget(status, inner_chunks[3]);
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
            sanitize_terminal_text("—")
        } else {
            self.state.status_panel.workspace_display.clone()
        };
        let summary = Line::from(vec![
            Span::styled(
                self.state.status_panel.backend_display.clone(),
                Style::default()
                    .fg(self.state.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(self.state.theme.muted)),
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
                StatusPanel::format_context_usage(
                    self.state.status_panel.input_context_tokens,
                    self.state.status_panel.context_window_tokens,
                    self.state.status_panel.input_context_tokens_estimated,
                ),
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
