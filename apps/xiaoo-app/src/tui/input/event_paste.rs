use anyhow::Result;

use crate::app::App;
use crate::app_state::InputMode;
use crate::interaction_prompt::PromptFocus;
use crate::render::paste_into_input;

impl App {
    pub(crate) fn handle_paste_event(&mut self, text: &str) -> Result<()> {
        if self.state.api_key_dialog.is_some() {
            let Some(mut dialog) = self.state.api_key_dialog.take() else {
                tracing::warn!("TUI: api key dialog state missing while handling paste event");
                self.state.input_mode = InputMode::Editing;
                return Ok(());
            };
            paste_into_input(&mut dialog.input, text);
            self.state.api_key_dialog = Some(dialog);
            return Ok(());
        }

        if let Some(prompt) = self.state.interaction_prompt.as_mut() {
            if prompt.focus == PromptFocus::Supplement {
                paste_into_input(&mut prompt.supplement, text);
            } else if prompt.request.allow_custom_input {
                prompt.focus = PromptFocus::Supplement;
                paste_into_input(&mut prompt.supplement, text);
            }
            return Ok(());
        }

        if self.state.input_mode == InputMode::Editing && self.state.provider_dialog.is_none() {
            paste_into_input(&mut self.state.chat_state.input, text);
            self.state.note_input_changed();
        }
        Ok(())
    }
}
