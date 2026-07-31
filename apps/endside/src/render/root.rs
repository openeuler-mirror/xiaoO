use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::App;

use super::utils::sanitize_terminal_text;

const LOADING_SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl App {
    pub fn loading_animation(&self) -> String {
        let spinner =
            LOADING_SPINNER_FRAMES[self.state.loading_tick % LOADING_SPINNER_FRAMES.len()];
        sanitize_terminal_text(&format!("{spinner} Thinking..."))
    }

    pub fn ui(&mut self, frame: &mut Frame) {
        #[cfg(debug_assertions)]
        let _ui_start = std::time::Instant::now();
        let size = frame.area();
        let is_loading = self.state.chat_state.is_loading;

        let (chunks, body_chunks, show_sidebar) = if self.state.render_state.cached_area == Some(size)
        {
            // Fast path: reuse cached layout (terminal has not resized).
            let chunks: &[Rect; 4] = self.state.render_state.cached_chunks.as_slice().try_into().unwrap();
            let body_chunks: &[Rect; 2] = self.state.render_state.cached_body_chunks.as_slice().try_into().unwrap();
            (chunks, body_chunks, self.state.render_state.cached_show_sidebar)
        } else {
            // Slow path: recompute layout and cache it.
            self.state.status_panel.set_workspace(&self.state.workspace);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(7),
                    Constraint::Length(3),
                ])
                .split(size);
            let body_area = chunks[1];
            let show_sidebar =
                body_area.width >= 72 || (self.state.plan_state.is_some() && body_area.width >= 60);
            let sidebar_width = (body_area.width / 3).clamp(28, 40);
            let body_chunks = if show_sidebar {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(32), Constraint::Length(sidebar_width)])
                    .split(body_area)
            } else {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(32)])
                    .split(body_area)
            };
            self.state.render_state.cached_area = Some(size);
            self.state.render_state.cached_chunks = chunks.to_vec();
            self.state.render_state.cached_body_chunks = body_chunks.to_vec();
            self.state.render_state.cached_show_sidebar = show_sidebar;
            (&self.state.render_state.cached_chunks.as_slice().try_into().unwrap(),
             &self.state.render_state.cached_body_chunks.as_slice().try_into().unwrap(),
             show_sidebar)
        };
        let chunks = *chunks;
        let body_chunks = *body_chunks;

        let background = Block::default().style(Style::default().bg(self.state.theme.background));
        frame.render_widget(background, size);

        #[cfg(debug_assertions)]
        let _t0 = std::time::Instant::now();
        self.render_header(frame, chunks[0]);
        #[cfg(debug_assertions)]
        let _t1 = std::time::Instant::now();
        self.render_chat(frame, body_chunks[0]);
        #[cfg(debug_assertions)]
        let _t2 = std::time::Instant::now();
        let input_chunk = chunks[2];
        self.state.render_state.interaction_prompt_list_area = None;
        self.state.render_state.interaction_prompt_supplement_area = None;
        self.state.render_state.slash_popup_inner = None;
        self.state.render_state.api_key_toggle_area = None;
        self.render_input(frame, input_chunk);
        #[cfg(debug_assertions)]
        let _t3 = std::time::Instant::now();
        let pending_bounds = Rect {
            x: body_chunks[0].x,
            y: size.y,
            width: body_chunks[0].width,
            height: input_chunk.y.saturating_sub(size.y),
        };
        self.render_pending_turns(frame, input_chunk, pending_bounds);
        if show_sidebar {
            self.render_sidebar(frame, body_chunks[1]);
        }
        #[cfg(debug_assertions)]
        let _t4 = std::time::Instant::now();
        self.render_status_bar(frame, chunks[3]);
        #[cfg(debug_assertions)]
        let _t5 = std::time::Instant::now();

        if self.state.provider_dialog.is_none()
            && self.state.sandbox_dialog.is_none()
            && self.state.remote_session_dialog.is_none()
            && self.state.api_key_dialog.is_none()
            && self.state.session_snapshot_dialog.is_none()
        {
            self.render_interaction_prompt_dialog(frame, frame.area());
            self.render_slash_popup_dialog(frame, frame.area());
        }
        if let Some(dialog) = self.state.provider_dialog.as_ref() {
            self.render_provider_dialog(frame, frame.area(), dialog);
        }
        if let Some(dialog) = self.state.sandbox_dialog.as_ref() {
            self.render_sandbox_dialog(frame, frame.area(), dialog);
        }
        if let Some(dialog) = self.state.remote_session_dialog.clone() {
            self.render_remote_session_dialog(frame, frame.area(), &dialog);
        }
        if let Some(dialog) = self.state.session_snapshot_dialog.as_ref() {
            self.render_session_snapshot_dialog(frame, frame.area(), dialog);
        }
        if let Some(dialog) = self.state.delete_dialog.as_ref() {
            self.render_delete_dialog(frame, frame.area(), dialog);
        }
        if let Some(dialog) = self.state.cron_dialog.clone() {
            self.render_cron_dialog(frame, frame.area(), &dialog);
        }
        if let Some(dialog) = self.state.api_key_dialog.clone() {
            self.render_api_key_dialog(frame, frame.area(), &dialog);
        }

        // Copy-to-clipboard toast (mirrors opencode's toast.show after copy).
        if self.state.copy_notice_active() {
            self.render_copy_toast(frame, size);
        }

        #[cfg(debug_assertions)]
        {
            // Use `Instant::duration_since` (saturates to zero on underflow)
            // instead of `Duration` subtraction, which panics on underflow.
            // The paired `Instant`s are nearly back-to-back when their guarded
            // render is skipped (e.g. loading), so the second `.elapsed()` can
            // exceed the first by a few ns — enough to underflow a naive `a - b`.
            let total = _ui_start.elapsed();
            let t_setup = _t0.duration_since(_ui_start);
            let t_header = _t1.duration_since(_t0);
            let t_chat = _t2.duration_since(_t1);
            let t_input = _t3.duration_since(_t2);
            let t_sidebar = _t4.duration_since(_t3);
            let t_status = _t5.duration_since(_t4);
            // dialogs = everything from _t5 (status_bar done) to now, including
            // the dialog overlay renders + copy_toast. Using `Instant::now()`
            // (not `_t5.elapsed() - total`) so the phases sum to `total`.
            let t_dialogs = std::time::Instant::now().duration_since(_t5);
            if total > std::time::Duration::from_micros(100) {
                eprintln!(
                    "PERF ui: total={}µs setup={}µs header={}µs chat={}µs input={}µs sidebar={}µs status={}µs dialogs={}µs loading={}",
                    total.as_micros(),
                    t_setup.as_micros(),
                    t_header.as_micros(),
                    t_chat.as_micros(),
                    t_input.as_micros(),
                    t_sidebar.as_micros(),
                    t_status.as_micros(),
                    t_dialogs.as_micros(),
                    is_loading,
                );
            }
        }
    }

    fn render_cron_dialog(
        &self,
        frame: &mut Frame,
        area: Rect,
        dialog: &crate::cron_dialog::CronDialog,
    ) {
        use super::utils::sanitize_terminal_text;
        use crate::cron_dialog::CronDialogMode;
        use ratatui::{
            layout::{Constraint, Direction, Layout},
            style::{Modifier, Style},
            text::{Line, Span},
            widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
        };

        let theme = self.state.theme;

        match &dialog.mode {
            CronDialogMode::List => {
                let dialog_width = (area.width * 3 / 4).min(80).max(40);
                let dialog_height = (area.height * 3 / 4).min(24).max(10);
                let x = (area.width.saturating_sub(dialog_width)) / 2;
                let y = (area.height.saturating_sub(dialog_height)) / 2;
                let dialog_area = Rect {
                    x,
                    y,
                    width: dialog_width,
                    height: dialog_height,
                };
                frame.render_widget(Clear, dialog_area);

                let title = if dialog.cron_section_present {
                    format!(
                        " Cron Jobs [{} job(s)] — a:add e:edit d:delete Space:toggle Esc:close ",
                        dialog.jobs.len()
                    )
                } else {
                    format!(" Cron Jobs [{} job(s)] (no [cron] section in config.toml) — a:add e:edit d:delete Space:toggle Esc:close ", dialog.jobs.len())
                };

                let block = Block::default()
                    .title(Span::styled(
                        sanitize_terminal_text(&title),
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .style(Style::default().fg(theme.foreground).bg(theme.background));
                frame.render_widget(block, dialog_area);

                let inner = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1)])
                    .margin(1)
                    .split(dialog_area)[0];

                if dialog.jobs.is_empty() {
                    let msg = Paragraph::new("No cron jobs configured. Press 'a' to add one.")
                        .style(Style::default().fg(theme.muted));
                    frame.render_widget(msg, inner);
                } else {
                    let items: Vec<ListItem> = dialog
                        .jobs
                        .iter()
                        .enumerate()
                        .map(|(i, job)| {
                            let prefix = if i == dialog.selected { "> " } else { "  " };
                            let status = if job.enabled { "[✓]" } else { "[ ]" };
                            let cron_status = if job.cron_valid { "" } else { " [INVALID]" };
                            let text = format!(
                                "{}{} {} ({}): {}   {}{}",
                                prefix,
                                status,
                                job.name,
                                job.cron_raw,
                                job.prompt.chars().take(40).collect::<String>(),
                                if job.prompt.len() > 40 { "..." } else { "" },
                                cron_status,
                            );
                            let style = if i == dialog.selected {
                                Style::default().fg(theme.foreground).bg(theme.selection)
                            } else if !job.enabled {
                                Style::default().fg(theme.muted)
                            } else if !job.cron_valid {
                                Style::default().fg(theme.error)
                            } else {
                                Style::default().fg(theme.foreground)
                            };
                            ListItem::new(sanitize_terminal_text(&text)).style(style)
                        })
                        .collect();

                    let list = List::new(items);
                    frame.render_widget(list, inner);
                }
            }

            CronDialogMode::ConfirmDelete { job_index } => {
                if let Some(job) = dialog.jobs.get(*job_index) {
                    let dialog_width = 60.min(area.width.saturating_sub(4));
                    let dialog_height = 6;
                    let x = (area.width.saturating_sub(dialog_width)) / 2;
                    let y = (area.height.saturating_sub(dialog_height)) / 2;
                    let dialog_area = Rect {
                        x,
                        y,
                        width: dialog_width,
                        height: dialog_height,
                    };
                    frame.render_widget(Clear, dialog_area);

                    let block = Block::default()
                        .title(Span::styled(
                            " Confirm Delete ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ))
                        .borders(Borders::ALL)
                        .style(Style::default().fg(theme.foreground).bg(theme.background));
                    frame.render_widget(block, dialog_area);

                    let inner = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(1), Constraint::Length(1)])
                        .margin(1)
                        .split(dialog_area);

                    let text = format!("Delete cron job '{}'? (y/N)", job.name);
                    let question = Paragraph::new(sanitize_terminal_text(&text))
                        .style(Style::default().fg(theme.secondary));
                    frame.render_widget(question, inner[0]);

                    let hint = Paragraph::new("Press 'y' to confirm, 'n' or Esc to cancel.")
                        .style(Style::default().fg(theme.muted));
                    frame.render_widget(hint, inner[1]);
                }
            }

            CronDialogMode::EditForm {
                form, focus, error, ..
            } => {
                let dialog_width = 70.min(area.width.saturating_sub(4));
                let dialog_height = (area.height.saturating_sub(2)).min(22).max(3);
                let x = (area.width.saturating_sub(dialog_width)) / 2;
                let y = (area.height.saturating_sub(dialog_height)) / 2;
                let dialog_area = Rect {
                    x,
                    y,
                    width: dialog_width,
                    height: dialog_height,
                };
                frame.render_widget(Clear, dialog_area);

                let title = " Edit Cron Job — Tab:next field  Enter:save  Esc:cancel ";
                let block = Block::default()
                    .title(Span::styled(
                        title,
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .style(Style::default().fg(theme.foreground).bg(theme.background));
                frame.render_widget(block, dialog_area);

                let inner = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1)])
                    .margin(1)
                    .split(dialog_area);

                let fields: Vec<(&str, &str, crate::cron_dialog::CronEditField)> = vec![
                    ("Name:", &form.name, crate::cron_dialog::CronEditField::Name),
                    ("Cron:", &form.cron, crate::cron_dialog::CronEditField::Cron),
                    (
                        "Prompt:",
                        &form.prompt,
                        crate::cron_dialog::CronEditField::Prompt,
                    ),
                    (
                        "Description:",
                        &form.description,
                        crate::cron_dialog::CronEditField::Description,
                    ),
                    (
                        "Agent Role:",
                        &form.agent_role,
                        crate::cron_dialog::CronEditField::AgentRole,
                    ),
                    (
                        "Timeout (s):",
                        &form.timeout_secs,
                        crate::cron_dialog::CronEditField::TimeoutSecs,
                    ),
                    (
                        "Max Retries:",
                        &form.max_retries,
                        crate::cron_dialog::CronEditField::MaxRetries,
                    ),
                    (
                        "Retry Delay (s):",
                        &form.retry_delay,
                        crate::cron_dialog::CronEditField::RetryDelay,
                    ),
                ];

                let mut lines: Vec<Line> = Vec::new();
                for (label, value, field) in &fields {
                    let is_focused = *focus == *field;
                    let label_style = if is_focused {
                        Style::default()
                            .fg(theme.foreground)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.muted)
                    };
                    let value_style = if is_focused {
                        Style::default().fg(theme.foreground).bg(theme.selection)
                    } else {
                        Style::default().fg(theme.foreground)
                    };
                    let display_value = if is_focused {
                        format!("{}█", sanitize_terminal_text(value))
                    } else {
                        sanitize_terminal_text(value)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:<14}", sanitize_terminal_text(label)),
                            label_style,
                        ),
                        Span::styled(display_value, value_style),
                    ]));
                }

                if let Some(err) = error {
                    lines.push(Line::from(Span::styled(
                        sanitize_terminal_text(&format!("Error: {}", err)),
                        Style::default()
                            .fg(theme.error)
                            .add_modifier(Modifier::BOLD),
                    )));
                }

                let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
                frame.render_widget(paragraph, inner[0]);
            }
        }
    }

    fn render_copy_toast(&self, frame: &mut Frame, area: Rect) {
        let message = " Copied to clipboard ";
        let width = message.chars().count() as u16;
        // Float in the bottom-right corner, just above the 3-row status bar.
        let x = area.x.saturating_add(area.width).saturating_sub(width + 1);
        let y = area.y.saturating_add(area.height).saturating_sub(4);
        let toast_area = Rect {
            x,
            y,
            width,
            height: 1,
        };
        let paragraph = Paragraph::new(message).style(
            Style::default()
                .fg(self.state.theme.background)
                .bg(self.state.theme.foreground)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(paragraph, toast_area);
    }
}
