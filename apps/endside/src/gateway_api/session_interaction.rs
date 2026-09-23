use async_trait::async_trait;
use xiaoo_api::interaction::{InteractionHandle, InteractionRequest, InteractionResponse};

use crate::interaction_prompt::UserPromptResult;

use super::interaction_convert::{build_prompt_request, map_response};
use super::session::{ChannelInteractionHandle, SessionTurnUpdate};

impl ChannelInteractionHandle {
    pub(super) fn new(
        updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>,
        interaction_rx: tokio::sync::mpsc::UnboundedReceiver<UserPromptResult>,
    ) -> Self {
        Self {
            updates_tx,
            interaction_rx: tokio::sync::Mutex::new(interaction_rx),
        }
    }
}

#[async_trait]
impl InteractionHandle for ChannelInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let prompt_request = build_prompt_request(request);
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::InteractionPrompt(prompt_request.clone()));

        let mut interaction_rx = self.interaction_rx.lock().await;
        while let Some(result) = interaction_rx.recv().await {
            if result.request_id != prompt_request.request_id {
                continue;
            }
            if let Some(response) = map_response(request, result) {
                return response;
            }
            break;
        }

        // Prompt channel closed or the user dismissed the prompt: give the
        // pending tool call a deny-style result so it can wind down.
        InteractionResponse::unanswered(request)
    }
}
