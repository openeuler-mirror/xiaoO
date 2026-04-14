use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    widgets::Block,
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
        self.render_input(frame, input_chunk);
        self.render_status_bar(frame, chunks[3]);

        if self.state.provider_dialog.is_some()
            || self.state.api_key_dialog.is_some()
            || self.state.interaction_prompt.is_some()
            || self.state.slash_menu_visible()
        {
            let overlay =
                Block::default().style(Style::default().bg(ratatui::style::Color::Rgb(5, 5, 10)));
            frame.render_widget(overlay, size);
        }

        if self.state.provider_dialog.is_none() && self.state.api_key_dialog.is_none() {
            self.render_interaction_prompt_dialog(frame, frame.area());
            self.render_slash_popup_dialog(frame, frame.area());
        }
        if let Some(dialog) = self.state.provider_dialog.as_ref() {
            self.render_provider_dialog(frame, frame.area(), dialog);
        }
        if let Some(dialog) = self.state.api_key_dialog.as_ref() {
            self.render_api_key_dialog(frame, frame.area(), dialog);
        }
    }
}
