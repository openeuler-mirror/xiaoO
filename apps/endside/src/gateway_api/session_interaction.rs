use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

use crate::interaction_prompt::{PromptChoice, PromptRequest, PromptResolution, UserPromptResult};

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

    fn build_prompt_request(request: &InteractionRequest) -> PromptRequest {
        match request {
            InteractionRequest::Confirm { prompt, .. } => PromptRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                title: prompt.clone(),
                body: None,
                choices: vec![
                    PromptChoice {
                        id: "approve".to_string(),
                        label: "Approve".to_string(),
                        description: None,
                    },
                    PromptChoice {
                        id: "reject".to_string(),
                        label: "Reject".to_string(),
                        description: None,
                    },
                ],
                allow_custom_input: false,
                multi_select: false,
                default_index: Some(0),
            },
            InteractionRequest::TextInput { prompt, .. } => PromptRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                title: prompt.clone(),
                body: None,
                choices: vec![PromptChoice {
                    id: "submit".to_string(),
                    label: "Submit".to_string(),
                    description: None,
                }],
                allow_custom_input: true,
                multi_select: false,
                default_index: Some(0),
            },
            InteractionRequest::Choice {
                prompt,
                options,
                allow_custom_input,
                ..
            } => PromptRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                title: prompt.clone(),
                body: None,
                choices: options
                    .iter()
                    .map(|option| PromptChoice {
                        id: option.clone(),
                        label: option.clone(),
                        description: None,
                    })
                    .collect(),
                allow_custom_input: *allow_custom_input,
                multi_select: false,
                default_index: Some(0),
            },
        }
    }

    fn map_response(
        request: &InteractionRequest,
        response: UserPromptResult,
    ) -> Option<InteractionResponse> {
        match (request, response.resolution) {
            (InteractionRequest::Confirm { .. }, PromptResolution::Single { choice_id, .. }) => {
                Some(InteractionResponse::Confirmed {
                    allowed: choice_id == "approve",
                })
            }
            (InteractionRequest::TextInput { .. }, PromptResolution::Single { supplement, .. }) => {
                Some(InteractionResponse::Text { value: supplement })
            }
            (
                InteractionRequest::Choice { .. },
                PromptResolution::Single {
                    choice_id,
                    supplement,
                },
            ) => Some(InteractionResponse::Choice {
                value: supplement.or(Some(choice_id)),
            }),
            (_, PromptResolution::Cancelled) => None,
            (_, PromptResolution::Multi { .. }) => None,
        }
    }
}

#[async_trait]
impl InteractionHandle for ChannelInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let prompt_request = Self::build_prompt_request(request);
        let _ = self
            .updates_tx
            .send(SessionTurnUpdate::InteractionPrompt(prompt_request.clone()));

        let mut interaction_rx = self.interaction_rx.lock().await;
        while let Some(result) = interaction_rx.recv().await {
            if result.request_id != prompt_request.request_id {
                continue;
            }
            if let Some(response) = Self::map_response(request, result) {
                return response;
            }
            break;
        }

        match request {
            InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
            InteractionRequest::TextInput { .. } => InteractionResponse::Text { value: None },
            InteractionRequest::Choice { .. } => InteractionResponse::Choice { value: None },
        }
    }
}
