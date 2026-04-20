use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::App;

const LOADING_SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl App {
    pub fn loading_animation(&self) -> String {
        let spinner =
            LOADING_SPINNER_FRAMES[self.state.loading_tick % LOADING_SPINNER_FRAMES.len()];
        format!("{} Thinking...", spinner)
    }

    pub fn ui(&mut self, frame: &mut Frame) {
        let size = frame.area();
        self.state.status_panel.set_workspace(&self.state.workspace);
        let background = Block::default().style(Style::default().bg(self.state.theme.background));
        frame.render_widget(background, size);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(7),
                Constraint::Length(3),
            ])
            .split(size);

        self.render_header(frame, chunks[0]);
        self.render_chat(frame, chunks[1]);
        let input_chunk = chunks[2];
        self.state.render_state.interaction_prompt_list_area = None;
        self.state.render_state.interaction_prompt_supplement_area = None;
        self.state.render_state.slash_popup_inner = None;
        self.state.render_state.api_key_toggle_area = None;
        self.render_input(frame, input_chunk);
        self.render_status_bar(frame, chunks[3]);

        if self.state.provider_dialog.is_none() && self.state.api_key_dialog.is_none() {
            self.render_interaction_prompt_dialog(frame, frame.area());
            self.render_slash_popup_dialog(frame, frame.area());
        }
        if let Some(dialog) = self.state.provider_dialog.as_ref() {
            self.render_provider_dialog(frame, frame.area(), dialog);
        }
        if let Some(dialog) = self.state.api_key_dialog.clone() {
            self.render_api_key_dialog(frame, frame.area(), &dialog);
        }

        // Copy-to-clipboard toast (mirrors opencode's toast.show after copy).
        if self.state.copy_notice_active() {
            self.render_copy_toast(frame, size);
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
