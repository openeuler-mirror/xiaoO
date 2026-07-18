use crate::httpserver::channel_ingress::{build_channel_turn_request, GatewayChannelMessage};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use std::sync::Arc;
use thiserror::Error;
use xiaoo_shared::gateway::{SessionService, SessionServiceError};

#[derive(Debug, Clone)]
pub struct GatewayTurnResponse {
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

    pub async fn handle_channel_message_with_interaction(
        &self,
        message: GatewayChannelMessage,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    ) -> Result<GatewayTurnResponse, GatewayServiceError> {
        let request = build_channel_turn_request(&message);
        let visible_reply = self
            .session_service
            .run_turn_with_interaction(
                request,
                event_sink,
                interaction_handle,
                channel_file_sender,
                None,
            )
            .await?
            .visible_reply;

        Ok(GatewayTurnResponse { visible_reply })
    }

    #[allow(dead_code)]
    pub async fn handle_channel_message_with_events(
        &self,
        message: GatewayChannelMessage,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<GatewayTurnResponse, GatewayServiceError> {
        self.handle_channel_message_with_interaction(message, event_sink, None, None)
            .await
    }
}
