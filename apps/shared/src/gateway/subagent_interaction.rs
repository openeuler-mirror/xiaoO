use agent_contracts::InteractionHandle;
use agent_types::common::ids::AgentId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::oneshot;

use super::session_supervisor::SessionSupervisor;

/// Sentinel text delivered to a subagent when its forwarded interaction
/// (`ask_user_question`) times out (either the gateway's outer
/// `interaction_timeout` fired, or the receiver was dropped before the
/// user replied). The text tells the model the user never replied so it
/// can stop waiting and wind the lane down instead of hanging forever.
pub(crate) const INTERACTION_TIMEOUT_SENTINEL: &str =
    "[INTERACTION_TIMEOUT] The user did not reply in time.";

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
        InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: Some(INTERACTION_TIMEOUT_SENTINEL.to_string()),
            display_value: None,
        },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice {
            value: Some(INTERACTION_TIMEOUT_SENTINEL.to_string()),
        },
    }
}

pub struct SubagentInteractionHandle {
    supervisor: Arc<SessionSupervisor>,
    agent_id: AgentId,
    parent_agent_id: AgentId,
}

impl SubagentInteractionHandle {
    pub fn new(
        supervisor: Arc<SessionSupervisor>,
        agent_id: AgentId,
        parent_agent_id: AgentId,
    ) -> Self {
        Self {
            supervisor,
            agent_id,
            parent_agent_id,
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
            )
            .await;

        response_rx
            .await
            .ok()
            .unwrap_or_else(|| interaction_timeout_response(request))
    }
}
