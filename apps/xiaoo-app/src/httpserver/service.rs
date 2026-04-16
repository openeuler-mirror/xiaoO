use crate::gateway::{AppTurnResult, SessionService, SessionServiceError};
use crate::httpserver::channel_ingress::{build_channel_turn_request, GatewayChannelMessage};
use agent_contracts::{InteractionHandle, LoopEventSink};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct GatewayTurnResponse {
    pub session_id: String,
    pub conversation_id: String,
    pub raw_reply: String,
    pub visible_reply: String,
}

pub struct GatewayService {
    session_service: Arc<dyn SessionService>,
}

#[derive(Debug, Error)]
pub enum GatewayServiceError {
    #[error("session service failed: {0}")]
    Session(#[from] SessionServiceError),
}

impl GatewayService {
    pub fn new(session_service: Arc<dyn SessionService>) -> Self {
        Self { session_service }
    }

    pub async fn handle_channel_message(
        &self,
        message: GatewayChannelMessage,
    ) -> Result<GatewayTurnResponse, GatewayServiceError> {
        self.handle_channel_message_with_interaction(message, None, None)
            .await
    }

    pub async fn handle_channel_message_with_interaction(
        &self,
        message: GatewayChannelMessage,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
    ) -> Result<GatewayTurnResponse, GatewayServiceError> {
        let request = build_channel_turn_request(&message);
        let session_id = request.session_id.clone();
        let conversation_id = request.conversation_id.clone();
        let AppTurnResult {
            raw_reply,
            visible_reply,
            messages: _messages,
            ..
        } = self
            .session_service
            .run_turn_with_interaction(request, event_sink, interaction_handle)
            .await?;

        Ok(GatewayTurnResponse {
            session_id,
            conversation_id,
            raw_reply,
            visible_reply,
        })
    }

    pub async fn handle_channel_message_with_events(
        &self,
        message: GatewayChannelMessage,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<GatewayTurnResponse, GatewayServiceError> {
        self.handle_channel_message_with_interaction(message, event_sink, None)
            .await
    }
}
