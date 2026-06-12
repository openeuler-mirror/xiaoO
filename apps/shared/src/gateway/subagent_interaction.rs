use agent_contracts::InteractionHandle;
use agent_types::common::ids::AgentId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::oneshot;

use super::session_supervisor::SessionSupervisor;

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
            .unwrap_or_else(|| default_timeout_response(request))
    }
}

fn default_timeout_response(request: &InteractionRequest) -> InteractionResponse {
    let sentinel = "[INTERACTION_TIMEOUT] The user did not reply in time.";
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: Some(sentinel.to_string()),
        },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice {
            value: Some(sentinel.to_string()),
        },
    }
}
