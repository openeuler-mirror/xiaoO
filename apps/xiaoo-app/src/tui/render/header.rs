use agent_types::ReasoningEffort;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
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
                Constraint::Length(20),
            ])
            .split(inner);

        let title = Paragraph::new(Line::from(vec![Span::styled(
            " XiaoO",
            Style::default()
                .fg(self.state.theme.accent)
                .add_modifier(Modifier::BOLD),
        )]));
        frame.render_widget(title, inner_chunks[0]);

        let all_labels = self.state.agent_tab_labels();
        let active_label = self.state.active_agent_tab_label().to_string();
        let active_index = all_labels
            .iter()
            .position(|label| label == &active_label)
            .unwrap_or(0);

        let available_width = inner_chunks[1].width as usize;

        let tab_widths: Vec<usize> = all_labels
            .iter()
            .map(|label| label.chars().count() + 2)
            .collect();

        let mut visible_start = self.state.render_state.first_visible_agent_tab;

        if active_index < visible_start {
            visible_start = active_index;
        } else {
            let mut test_start = visible_start;

            while test_start <= active_index {
                let mut total_width = 0;
                let mut last_visible_index = test_start;

                for (i, width) in tab_widths[test_start..].iter().enumerate() {
                    let w = if total_width > 0 { width + 1 } else { *width };
                    if total_width + w > available_width {
                        break;
                    }
                    total_width += w;
                    last_visible_index = test_start + i;
                }

                if last_visible_index >= active_index {
                    break;
                }

                test_start += 1;
            }

            visible_start = test_start;
        }

        self.state.render_state.first_visible_agent_tab = visible_start;

        let mut tabs = Vec::new();
        let mut current_width = 0;

        for (index, label) in all_labels.iter().enumerate() {
            if index < visible_start {
                continue;
            }

            let tab_width = label.chars().count() + 2;
            let needs_space = current_width > 0;
            let additional_width = if needs_space { tab_width + 1 } else { tab_width };

            if current_width + additional_width > available_width {
                break;
            }

            if needs_space {
                tabs.push(Span::raw(" "));
                current_width += 1;
            }

            tabs.push(Span::styled(
                format!(" {label} "),
                self.state.theme.tab_style(label == &active_label),
            ));
            current_width += tab_width;
        }

        if let Some(role) = self.state.active_agent_role_config() {
            if !role.description.trim().is_empty() {
                let desc = role.description.as_str();
                let desc_width = desc.chars().count() + 2;
                if current_width + desc_width <= available_width {
                    tabs.push(Span::raw("  "));
                    tabs.push(Span::styled(
                        desc,
                        Style::default().fg(self.state.theme.muted),
                    ));
                }
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

        let now = chrono::Local::now();
        let now_text = now.format("%H:%M:%S").to_string();
        let (status_light_symbol, status_light_style, status_label, status_label_style) =
            match self.state.runtime_status_light() {
                RuntimeStatusLight::Running => {
                    let (symbol, light_style) =
                        running_status_light(now.timestamp_millis(), self.state.theme.success);
                    (
                        symbol,
                        light_style,
                        "RUN",
                        Style::default()
                            .fg(self.state.theme.success)
                            .add_modifier(Modifier::BOLD),
                    )
                }
                RuntimeStatusLight::AwaitingInteraction => (
                    "●",
                    Style::default().fg(self.state.theme.gradient_yellow),
                    "ASK",
                    Style::default()
                        .fg(self.state.theme.gradient_yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                RuntimeStatusLight::Idle => (
                    "●",
                    Style::default().fg(self.state.theme.foreground),
                    "IDLE",
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
            };
        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                sanitize_terminal_text(&format!("{status_light_symbol} ")),
                status_light_style,
            ),
            Span::styled(format!("{status_label} "), status_label_style),
            Span::styled(now_text, Style::default().fg(self.state.theme.muted)),
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
            Span::styled(
                "  Think(Ctrl+T) ",
                Style::default().fg(self.state.theme.muted),
            ),
            Span::styled(
                self.state.reasoning_effort.to_string(),
                Style::default()
                    .fg(reasoning_effort_color(self.state.reasoning_effort))
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let summary_bar = Paragraph::new(summary);
        frame.render_widget(summary_bar, inner);
    }
}

fn reasoning_effort_color(effort: ReasoningEffort) -> Color {
    match effort {
        ReasoningEffort::Off => Color::Gray,
        ReasoningEffort::High => Color::Yellow,
        ReasoningEffort::Max => Color::Red,
    }
}

fn running_status_light(now_millis: i64, color: Color) -> (&'static str, Style) {
    let bright = now_millis.rem_euclid(1_200) < 650;
    let symbol = if bright { "●" } else { "○" };
    let modifier = if bright {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    (symbol, Style::default().fg(color).add_modifier(modifier))
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier};

    use super::running_status_light;

    #[test]
    fn running_status_light_blinks_at_human_scale() {
        let (bright_symbol, bright_style) = running_status_light(0, Color::Green);
        let (dim_symbol, dim_style) = running_status_light(700, Color::Green);

        assert_eq!(bright_symbol, "●");
        assert!(bright_style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(dim_symbol, "○");
        assert!(!dim_style.add_modifier.contains(Modifier::BOLD));
    }
}
