use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation},
    Frame,
};

use crate::app::App;
use crate::chat::{TodoDisplayStatus, TodoMessageState};
use crate::render::utils::{display_width, sanitize_terminal_text, truncate_display_width};
use crate::render::wrap_line_to_visual_lines;

impl App {
    pub(crate) fn render_sidebar(&mut self, frame: &mut Frame, area: Rect) {
        // Clone the plan snapshot so `render_plan_panel` can update the
        // panel's scroll bookkeeping (`&mut self.state.plan_panel`) without
        // borrowing the plan out of `self.state` at the same time.
        let Some(plan) = self.state.plan_state.clone() else {
            self.state.render_state.plan_panel_area = None;
            self.render_session_diff(frame, area);
            return;
        };

        if area.height < 12 {
            self.render_plan_panel(frame, area, &plan);
            return;
        }

        let plan_height = plan_panel_height(&plan, area.width, area.height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(plan_height), Constraint::Min(5)])
            .split(area);
        self.render_plan_panel(frame, chunks[0], &plan);
        self.render_session_diff(frame, chunks[1]);
    }

    fn render_plan_panel(&mut self, frame: &mut Frame, area: Rect, plan: &TodoMessageState) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.state.theme.border))
            .title(" Plan ")
            .style(Style::default().bg(self.state.theme.background));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Scrollbar track area (outer x/width, inner y/height) — the same
        // convention as `messages_area` in the transcript panel, so mouse
        // hit-testing (wheel scroll + thumb drag) behaves identically. The
        // thumb is drawn over the right border, keeping the full inner
        // width available for text.
        let scrollbar_area = Rect {
            x: area.x,
            y: inner.y,
            width: area.width,
            height: inner.height,
        };
        self.state.render_state.plan_panel_area = Some(scrollbar_area);

        if inner.width == 0 || inner.height == 0 {
            // Panel too small to show content: zero the scroll bookkeeping
            // so stale totals from a previous frame don't keep it scrollable.
            self.state.plan_panel.total_lines = 0;
            self.state.plan_panel.last_visible_height = 0;
            self.state.plan_panel.scroll_offset = 0;
            return;
        }

        let completed = plan
            .items
            .iter()
            .filter(|(status, _)| *status == TodoDisplayStatus::Completed)
            .count();
        let total = plan.items.len();

        // Build the full wrapped content: header + blank separator + items.
        // Overlong lines wrap (with a hanging indent under the item text)
        // instead of being truncated with an ellipsis, and the panel scrolls
        // vertically when the wrapped rows exceed the available height.
        let content_width = inner.width as usize;
        let mut visual_lines = wrap_line_to_visual_lines(
            &Line::from(vec![
                Span::styled(
                    format!("{completed}/{total} done"),
                    Style::default()
                        .fg(self.state.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    sanitize_terminal_text(&plan.title),
                    Style::default().fg(self.state.theme.muted),
                ),
            ]),
            inner.width,
        );
        visual_lines.push(Line::raw(""));

        for (status, content) in &plan.items {
            let (marker, color) = match status {
                TodoDisplayStatus::Completed => ("✓", self.state.theme.success),
                TodoDisplayStatus::InProgress => (">", self.state.theme.accent),
                TodoDisplayStatus::Pending => (" ", self.state.theme.muted),
            };
            visual_lines.extend(plan_item_visual_lines(
                marker,
                color,
                self.state.theme.foreground,
                content,
                content_width,
            ));
        }

        // Refresh scroll bookkeeping against the freshly wrapped content:
        // total rows and viewport height can change every frame (sidebar
        // resize, new plan snapshot), so clamp the offset to stay in range.
        let visible_height = inner.height as usize;
        let total_lines = visual_lines.len();
        let plan_panel = &mut self.state.plan_panel;
        plan_panel.total_lines = total_lines;
        plan_panel.last_visible_height = visible_height;
        let max_scroll = plan_panel.max_scroll_offset();
        plan_panel.scroll_offset = plan_panel.scroll_offset.min(max_scroll);
        plan_panel.sync_scrollbar_state();

        let start = self.state.plan_panel.scroll_offset;
        let end = start.saturating_add(visible_height).min(total_lines);
        frame.render_widget(
            Paragraph::new(visual_lines[start..end].to_vec()),
            inner,
        );

        if max_scroll > 0 {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .style(Style::default().fg(self.state.theme.border));
            frame.render_stateful_widget(
                scrollbar,
                scrollbar_area,
                &mut self.state.plan_panel.scrollbar_state,
            );
        }
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

/// Build the wrapped visual rows for one plan item: a colored
/// `[✓] `/`[>] `/`[ ] ` marker followed by the sanitized content, word-
/// wrapped to `content_width` with continuation rows indented under the
/// item text (hanging indent) instead of being truncated with an ellipsis.
fn plan_item_visual_lines(
    marker: &str,
    marker_color: Color,
    foreground: Color,
    content: &str,
    content_width: usize,
) -> Vec<Line<'static>> {
    let prefix = sanitize_terminal_text(&format!("[{marker}] "));
    let prefix_width = display_width(&prefix);
    let content = sanitize_terminal_text(content);
    if content.trim().is_empty() {
        // Wrapping an empty string yields no rows; still render the marker
        // row so the item stays visible.
        return vec![Line::from(Span::styled(
            prefix,
            Style::default().fg(marker_color),
        ))];
    }
    // Continuation rows are indented by the prefix width so wrapped text
    // aligns under the item text, not under the marker itself.
    let indent_width = prefix_width.min(content_width.saturating_sub(1));
    let item_width = content_width.saturating_sub(prefix_width).max(1);
    wrap_line_to_visual_lines(
        &Line::from(Span::styled(content, Style::default().fg(foreground))),
        item_width as u16,
    )
    .into_iter()
    .enumerate()
    .map(|(row, mut wrapped)| {
        let lead = if row == 0 {
            Span::styled(prefix.clone(), Style::default().fg(marker_color))
        } else {
            Span::raw(" ".repeat(indent_width))
        };
        wrapped.spans.insert(0, lead);
        wrapped
    })
    .collect()
}

fn plan_panel_height(plan: &TodoMessageState, sidebar_width: u16, available_height: u16) -> u16 {
    if available_height < 14 {
        return available_height;
    }
    // Borders take 2 columns, so the panel's text area is 2 narrower than
    // the sidebar. Guard against degenerate widths (>= 3 keeps a >= 1 wide
    // text area, matching the old clamp floor of 7 rows incl. borders).
    let inner_width = sidebar_width.saturating_sub(2);
    if inner_width == 0 {
        return 7.min(available_height);
    }
    // Desired height = actual wrapped content height (header + separator +
    // wrapped item rows), so the panel grows to fit un-truncated items up to
    // half the body height and scrolls beyond that. Reuses the exact same
    // wrapping helper the renderer uses, so the prediction can never
    // disagree with what `render_plan_panel` actually draws.
    let mut desired: u16 = 2; // header row + blank separator
    for (status, content) in &plan.items {
        let marker = match status {
            TodoDisplayStatus::Completed => "✓",
            TodoDisplayStatus::InProgress => ">",
            TodoDisplayStatus::Pending => " ",
        };
        // Colors don't affect wrapping; placeholders are fine for measuring.
        let rows = plan_item_visual_lines(
            marker,
            Color::Reset,
            Color::Reset,
            content,
            inner_width as usize,
        )
        .len();
        desired = desired.saturating_add(rows.max(1) as u16);
    }
    // +2 for the top/bottom borders.
    let desired = desired.saturating_add(2);
    desired.clamp(7, (available_height / 2).max(7))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(content: &str) -> TodoMessageState {
        TodoMessageState {
            title: "Implement plan panel scrolling".to_string(),
            items: vec![(TodoDisplayStatus::Pending, content.to_string())],
        }
    }

    /// Joined text of a rendered row (all spans concatenated).
    fn row_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn short_item_fits_on_one_row_with_marker() {
        let rows = plan_item_visual_lines("✓", Color::Green, Color::Gray, "Fix login bug", 30);
        assert_eq!(rows.len(), 1);
        assert_eq!(row_text(&rows[0]), "[✓] Fix login bug");
    }

    #[test]
    fn long_item_wraps_instead_of_truncating() {
        let content = "Refactor the authentication flow so that login failures retry with backoff and surface a clear user-facing error message after exhausting the retries";
        let rows = plan_item_visual_lines(" ", Color::DarkGray, Color::Gray, content, 30);
        assert!(
            rows.len() > 1,
            "long content must wrap onto multiple rows, got {}",
            rows.len()
        );
        // No ellipsis truncation: every character of the content is still
        // present across the wrapped rows (whitespace-insensitive compare).
        let rendered: String = rows
            .iter()
            .map(|l| row_text(l))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let expected: String = format!("[ ] {content}")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert_eq!(rendered, expected);
    }

    #[test]
    fn wrapped_continuation_rows_are_indented_under_content() {
        let content = "one two three four five six seven eight nine ten eleven twelve thirteen";
        let rows = plan_item_visual_lines(">", Color::Cyan, Color::Gray, content, 24);
        assert!(rows.len() >= 2, "content must wrap at width 24");
        let first = row_text(&rows[0]);
        assert!(first.starts_with("[>] "));
        let indent = " ".repeat(display_width(&sanitize_terminal_text("[>] ")));
        for continuation in &rows[1..] {
            let text = row_text(continuation);
            assert!(
                text.starts_with(&indent),
                "continuation row must be indented under the item text, got {text:?}"
            );
        }
    }

    #[test]
    fn wrapped_rows_never_exceed_content_width() {
        let content = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let rows = plan_item_visual_lines("✓", Color::Green, Color::Gray, content, 30);
        assert!(rows.len() > 1);
        for row in &rows {
            let width: usize = row.spans.iter().map(|s| display_width(&s.content)).sum();
            assert!(
                width <= 30,
                "row wider than the content area: {width} > 30 ({:?})",
                row_text(row)
            );
        }
    }

    #[test]
    fn empty_item_still_renders_marker_row() {
        let rows = plan_item_visual_lines(" ", Color::DarkGray, Color::Gray, "", 30);
        assert_eq!(rows.len(), 1);
        // The prefix keeps its trailing space (marker column layout).
        assert_eq!(row_text(&rows[0]), "[ ] ");
    }

    #[test]
    fn plan_panel_height_grows_for_wrapped_items() {
        let long = plan(
            "Refactor the authentication flow so that login failures retry with backoff and surface a clear user-facing error message",
        );
        // A narrower sidebar wraps the same item onto more rows, so the
        // panel must reserve more height.
        let narrow = plan_panel_height(&long, 28, 40);
        let wide = plan_panel_height(&long, 60, 40);
        assert!(
            narrow > wide,
            "narrower sidebar must reserve more rows: {narrow} vs {wide}"
        );
        // Both stay within the half-height cap.
        assert!(narrow <= 20, "height must respect the half-body cap");
    }

    #[test]
    fn plan_panel_height_respects_bounds() {
        // A short list reserves exactly the 7-row minimum: borders(2) +
        // header(1) + blank(1) + 3 single-row items.
        let small = TodoMessageState {
            title: "t".to_string(),
            items: vec![
                (TodoDisplayStatus::Pending, "a".to_string()),
                (TodoDisplayStatus::Pending, "b".to_string()),
                (TodoDisplayStatus::Pending, "c".to_string()),
            ],
        };
        assert_eq!(plan_panel_height(&small, 30, 40), 7);

        // Many items exceed half of 40 → capped at 20.
        let big = TodoMessageState {
            title: "t".to_string(),
            items: (0..30)
                .map(|i| (TodoDisplayStatus::Pending, format!("task {i}")))
                .collect(),
        };
        assert_eq!(plan_panel_height(&big, 30, 40), 20);

        // Tiny available height passes through unchanged.
        assert_eq!(plan_panel_height(&big, 30, 10), 10);
    }

    #[test]
    fn plan_panel_height_never_exceeds_actual_wrapped_content() {
        // Height prediction must equal what the renderer draws: borders +
        // header + blank + wrapped rows (unclamped when below the cap).
        let p = plan("a short item");
        // 2 borders + 1 header + 1 blank + 1 item row = 5 → clamped to min 7.
        assert_eq!(plan_panel_height(&p, 30, 40), 7);
        // With 8 one-row items: 2 + 1 + 1 + 8 = 12 rows.
        let p8 = TodoMessageState {
            title: "t".to_string(),
            items: (0..8)
                .map(|i| (TodoDisplayStatus::Pending, format!("item {i}")))
                .collect(),
        };
        assert_eq!(plan_panel_height(&p8, 60, 80), 12);
    }
}
