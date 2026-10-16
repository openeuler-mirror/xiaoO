use agent_contracts::InteractionHandle;
use agent_types::common::ids::AgentId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::session_supervisor::SessionSupervisor;

/// Sentinel text delivered to a subagent when its forwarded interaction
/// (`ask_user_question`) times out (either the gateway's outer
/// `interaction_timeout` fired, or the receiver was dropped before the
/// user replied). The text tells the model the user never replied so it
/// can stop waiting and wind the lane down instead of hanging forever.
pub(crate) const INTERACTION_TIMEOUT_SENTINEL: &str =
    "[INTERACTION_TIMEOUT] The user did not reply in time.";

/// Sentinel text delivered to a subagent when its forwarded interaction
/// (`ask_user_question` / a permission confirm) is cut short because the
/// root turn was cancelled (Esc/Ctrl+C) before the user replied. Distinct
/// from [`INTERACTION_TIMEOUT_SENTINEL`] so the persisted history shows
/// why no real answer arrived.
pub(crate) const INTERACTION_CANCELLED_SENTINEL: &str =
    "[INTERACTION_CANCELLED] The turn was cancelled before the user replied.";

/// Build the sentinel [`InteractionResponse`] for a timed-out forwarded
/// interaction. Shared between:
/// - [`SubagentInteractionHandle::ask`]'s `response_rx` failure fallback
///   (the `oneshot::Receiver` returned `None`, i.e. the `Sender` was
///   dropped without sending — happens when the gateway's outer timeout
///   aborts the spawned `request_interaction` task).
/// - [`SessionSupervisor::request_interaction`]'s outer `select!` branch
///   when the configured `interaction_timeout` fires before the user
///   replies.
pub(crate) fn interaction_timeout_response(request: &InteractionRequest) -> InteractionResponse {
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Unanswered,
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: Some(INTERACTION_TIMEOUT_SENTINEL.to_string()),
            display_value: None,
        },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice {
            value: Some(INTERACTION_TIMEOUT_SENTINEL.to_string()),
        },
    }
}

/// Build the sentinel [`InteractionResponse`] for a cancelled forwarded
/// interaction: delivered by [`SessionSupervisor::request_interaction`]'s
/// cancel `select!` branch when the root turn's token fires while the
/// user has not replied yet. Mirrors [`interaction_timeout_response`]:
/// `Confirm` maps to `Unanswered` (not an explicit denial), free-text
/// variants carry the sentinel so the model can wind down.
pub(crate) fn interaction_cancelled_response(request: &InteractionRequest) -> InteractionResponse {
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Unanswered,
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: Some(INTERACTION_CANCELLED_SENTINEL.to_string()),
            display_value: None,
        },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice {
            value: Some(INTERACTION_CANCELLED_SENTINEL.to_string()),
        },
    }
}

pub(crate) struct SubagentInteractionHandle {
    supervisor: Arc<SessionSupervisor>,
    agent_id: AgentId,
    parent_agent_id: AgentId,
    /// The root turn's cancel token, captured when the subagent lane was
    /// spawned (see `spawn_subagent_task`). Forwarded to
    /// `request_interaction` so a pending `ask` unparks the moment the
    /// turn is cancelled, instead of waiting out the interaction timeout.
    cancellation_token: Option<CancellationToken>,
}

impl SubagentInteractionHandle {
    pub(crate) fn new(
        supervisor: Arc<SessionSupervisor>,
        agent_id: AgentId,
        parent_agent_id: AgentId,
        cancellation_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            supervisor,
            agent_id,
            parent_agent_id,
            cancellation_token,
        }
    }
}

#[async_trait]
impl InteractionHandle for SubagentInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();

        self.supervisor
            .request_interaction(
                request_id,
                self.agent_id.clone(),
                self.parent_agent_id.clone(),
                request.clone(),
                response_tx,
                self.cancellation_token.clone(),
            )
            .await;

        response_rx
            .await
            .ok()
            .unwrap_or_else(|| interaction_timeout_response(request))
    }
}
