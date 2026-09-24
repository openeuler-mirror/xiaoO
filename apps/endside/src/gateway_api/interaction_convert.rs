//! Shared conversion between agent-side `InteractionRequest` /
//! `InteractionResponse` and TUI-side `PromptRequest` / `UserPromptResult`,
//! used by both the remote SSE and local session paths.

use xiaoo_api::interaction::{InteractionRequest, InteractionResponse, InteractionSource};

use crate::interaction_prompt::{
    PromptChoice, PromptRequest, PromptResolution, UserPromptResult, TYPE_ANSWER_HINT,
};

/// Field defaults shared by every prompt flavor.
fn base_prompt_request(title: String) -> PromptRequest {
    PromptRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        title,
        body: None,
        choices: Vec::new(),
        allow_custom_input: false,
        custom_input_hint: None,
        multi_select: false,
        default_index: Some(0),
        is_secret: false,
    }
}

pub(super) fn build_prompt_request(request: &InteractionRequest) -> PromptRequest {
    match request {
        InteractionRequest::Confirm { prompt, .. } => PromptRequest {
            choices: vec![
                PromptChoice {
                    id: "yes".to_string(),
                    label: "Yes".to_string(),
                    description: None,
                },
                PromptChoice {
                    id: "no".to_string(),
                    label: "No".to_string(),
                    description: None,
                },
            ],
            ..base_prompt_request(prompt.clone())
        },
        InteractionRequest::TextInput {
            prompt, is_secret, ..
        } => PromptRequest {
            choices: vec![PromptChoice {
                id: "submit".to_string(),
                label: "Submit".to_string(),
                description: None,
            }],
            allow_custom_input: true,
            is_secret: *is_secret,
            ..base_prompt_request(prompt.clone())
        },
        InteractionRequest::Choice {
            prompt,
            options,
            allow_custom_input,
            source,
        } => {
            // Flag maps faithfully: permission backends only accept their
            // exact option strings; `ask_user_question` sends `true` always.
            let tool_sourced = matches!(source, Some(InteractionSource::Tool { .. }));
            PromptRequest {
                choices: options
                    .iter()
                    .map(|option| PromptChoice {
                        id: option.clone(),
                        label: option.clone(),
                        description: None,
                    })
                    .collect(),
                allow_custom_input: *allow_custom_input,
                custom_input_hint: (*allow_custom_input && tool_sourced)
                    .then(|| TYPE_ANSWER_HINT.to_string()),
                ..base_prompt_request(prompt.clone())
            }
        }
    }
}

pub(super) fn map_response(
    request: &InteractionRequest,
    response: UserPromptResult,
) -> Option<InteractionResponse> {
    match (request, response.resolution) {
        (InteractionRequest::Confirm { .. }, PromptResolution::Single { choice_id, .. }) => {
            Some(InteractionResponse::Confirmed {
                allowed: choice_id == "yes",
            })
        }
        (
            InteractionRequest::TextInput { is_secret, .. },
            PromptResolution::Single { supplement, .. },
        ) => {
            let display_value = if *is_secret {
                Some("<SECRET>".to_string())
            } else {
                None
            };
            Some(InteractionResponse::Text {
                value: supplement,
                display_value,
            })
        }
        (
            InteractionRequest::Choice { .. },
            PromptResolution::Single {
                choice_id,
                supplement,
                submitted_from_supplement,
            },
        ) => {
            // A typed answer wins only when submitted from the supplement
            // box; otherwise it is stale residue that must not override the
            // selection.
            let value = if submitted_from_supplement {
                supplement.or(Some(choice_id))
            } else {
                Some(choice_id)
            };
            Some(InteractionResponse::Choice { value })
        }
        (_, PromptResolution::Cancelled) => None,
        // Unreachable: `build_prompt_request` never sets `multi_select: true`.
        (_, PromptResolution::Multi { .. }) => {
            debug_assert!(
                false,
                "multi-select prompt results are not produced by build_prompt_request"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/gateway_api/interaction_convert_test.rs"]
mod tests;
