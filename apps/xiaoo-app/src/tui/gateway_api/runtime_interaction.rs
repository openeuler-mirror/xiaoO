use crate::app_state::{AppState, InputMode};
use crate::chat::Message;
use crate::interaction_prompt::{PromptResolution, UserPromptResult};

use super::runtime::GatewayRuntime;

impl GatewayRuntime {
    pub fn resolve_interaction_prompt(
        &mut self,
        state: &mut AppState,
        resolution: PromptResolution,
    ) {
        let request_id = state
            .interaction_prompt
            .as_ref()
            .map(|prompt| prompt.request.request_id.clone());
        if let (Some(tx), Some(id)) = (self.interaction_reply_tx.as_ref(), request_id.clone()) {
            let _ = tx.send(UserPromptResult {
                request_id: id,
                resolution,
            });
        } else if let Some(id) = request_id {
            let result = UserPromptResult {
                request_id: id,
                resolution,
            };
            if let Ok(json) = serde_json::to_string(&result) {
                state.chat_state.messages.push(Message::system(json));
                state.chat_state.stick_to_bottom = true;
            }
        }
        state.interaction_prompt = None;
        state.input_mode = InputMode::Editing;
    }
}
