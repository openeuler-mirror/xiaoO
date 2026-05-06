use crate::gateway::{
    AppTurnRequest, AppTurnResult, SessionOpenRequest, SessionRecord, SessionStreamMode,
    SessionSubmitReceipt, SessionSubscription,
};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionServiceError {
    #[error("session store failed: {message}")]
    SessionStore { message: String },
    #[error("runtime resolution failed: {message}")]
    RuntimeResolve { message: String },
    #[error("runtime build failed: {message}")]
    RuntimeBuild { message: String },
    #[error("runtime shutdown failed: {message}")]
    RuntimeShutdown { message: String },
    #[error("core runtime execution failed: {message}")]
    CoreRun { message: String },
    #[error("memory handling failed: {message}")]
    Memory { message: String },
    #[error("unsupported capability: {capability}")]
    UnsupportedCapability { capability: String },
}

#[async_trait]
pub trait SessionService: Send + Sync {
    async fn run_turn(&self, request: AppTurnRequest)
        -> Result<AppTurnResult, SessionServiceError>;

    async fn run_turn_with_events(
        &self,
        request: AppTurnRequest,
        _event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn(request).await
    }

    async fn run_turn_with_interaction(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        _interaction_handle: Option<Arc<dyn InteractionHandle>>,
        _channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_with_events(request, event_sink).await
    }
}

#[async_trait]
pub trait SessionControlPlane: Send + Sync {
    async fn open_session(
        &self,
        _request: SessionOpenRequest,
    ) -> Result<SessionRecord, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "open_session".to_string(),
        })
    }

    async fn resume_session(
        &self,
        _session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "resume_session".to_string(),
        })
    }

    async fn force_close_session(
        &self,
        _session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "force_close_session".to_string(),
        })
    }

    async fn submit_input(
        &self,
        _session_id: &str,
        _input: crate::gateway::SessionInput,
    ) -> Result<SessionSubmitReceipt, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "submit_input".to_string(),
        })
    }

    async fn subscribe(
        &self,
        _session_id: &str,
        _mode: SessionStreamMode,
    ) -> Result<SessionSubscription, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "subscribe".to_string(),
        })
    }
}
