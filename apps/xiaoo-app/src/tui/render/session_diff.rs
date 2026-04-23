use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::render::utils::truncate_display_width;

impl App {
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
