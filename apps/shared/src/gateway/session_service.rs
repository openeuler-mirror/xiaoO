use crate::gateway::{
    AppTurnRequest, AppTurnResult, SessionOpenRequest, SessionRecord, SessionSubmitReceipt,
};
use crate::{
    RuntimeCheckoutRequest, RuntimeCheckoutResult, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, RuntimeCheckpointSnapshotDeleteRequest,
    RuntimeCheckpointSnapshotDeleteResult, RuntimePauseRequest, RuntimePauseResult,
    RuntimeResumeRequest, RuntimeResumeResult,
};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use async_trait::async_trait;
use memory::MemorySnapshot;
use std::sync::Arc;
use thiserror::Error;
use tool::ToolSpecSnapshot;
use xiaoo_core::LoopStateSnapshot;

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
    #[error("core runtime execution failed with partial state: {message}")]
    CoreRunWithState {
        message: String,
        partial_loop_state: LoopStateSnapshot,
        partial_memory_snapshot: MemorySnapshot,
        tool_manifest: Vec<ToolSpecSnapshot>,
    },
    #[error("memory handling failed: {message}")]
    Memory { message: String },
    #[error("unsupported capability: {capability}")]
    UnsupportedCapability { capability: String },
    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },
    #[error("session busy: {session_id}: {message}")]
    SessionBusy { session_id: String, message: String },
    #[error("session closed: {session_id}")]
    SessionClosed { session_id: String },
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
    ) -> Result<SessionRecord, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "force_close_session".to_string(),
        })
    }

    async fn checkpoint_runtime(
        &self,
        _request: RuntimeCheckpointRequest,
    ) -> Result<RuntimeCheckpointResult, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "checkpoint_runtime".to_string(),
        })
    }

    async fn checkout_runtime(
        &self,
        _request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutResult, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "checkout_runtime".to_string(),
        })
    }

    async fn pause_runtime(
        &self,
        _request: RuntimePauseRequest,
    ) -> Result<RuntimePauseResult, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "pause_runtime".to_string(),
        })
    }

    async fn resume_runtime(
        &self,
        _request: RuntimeResumeRequest,
    ) -> Result<RuntimeResumeResult, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "resume_runtime".to_string(),
        })
    }

    async fn delete_checkpoint_snapshot(
        &self,
        _request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteResult, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "delete_checkpoint_snapshot".to_string(),
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
}
