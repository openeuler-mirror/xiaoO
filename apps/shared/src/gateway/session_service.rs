use crate::gateway::{AppTurnRequest, AppTurnResult, SessionOpenRequest, SessionRecord};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use async_trait::async_trait;
use memory::MemorySnapshot;
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tool::ToolSpecSnapshot;
use xiaoo_core::LoopStateSnapshot;

#[derive(Debug, Error)]
pub enum SessionServiceError {
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("runtime conflict: {message}")]
    RuntimeConflict { message: String },
    #[error("request payload too large: {message}")]
    PayloadTooLarge { message: String },
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
    #[error(
        "session attached by another client: {session_id} (held by {holder_client_id}@{holder_hostname} pid={holder_pid}, last_heartbeat_ms={last_heartbeat_ms}, stale={stale})"
    )]
    SessionAttachedByAnotherClient {
        session_id: String,
        holder_client_id: String,
        holder_hostname: String,
        holder_pid: u32,
        last_heartbeat_ms: u64,
        /// `true` when `now - last_heartbeat_ms > STALE_LEASE_THRESHOLD_MS` at
        /// the moment the error was constructed. Lets a TUI decide whether to
        /// auto-retry (stale → safe to take over on the next `acquire`) or
        /// surface the error to the user (live → another process holds the
        /// session; the user must stop it before retrying).
        stale: bool,
    },
    /// Returned by lease guards when the daemon requires an identified
    /// `client_id` for mutating RPCs but the request omitted one. HTTP 401
    /// (so a misconfigured TUI gets a clear "client_id required" message).
    #[error("lease required: anonymous callers are not permitted on session {session_id}")]
    LeaseRequired { session_id: String },
    /// Daemon wall clock is before `UNIX_EPOCH`. Lease enforcement is
    /// fail-closed — `acquire` / `heartbeat` / holder checks refuse to
    /// proceed rather than silently relaxing single-writer. HTTP 503 so the
    /// operator notices and the TUI can retry once the clock is corrected.
    #[error("lease clock skew: daemon wall clock is before UNIX_EPOCH; lease enforcement is fail-closed for session {session_id}")]
    LeaseClockSkew { session_id: String },
}

impl SessionServiceError {
    /// Convenience constructor for the
    /// `SessionAttachedByAnotherClient` variant from a `LeaseCheckFailure`.
    pub(crate) fn from_lease_check_failure(
        session_id: impl Into<String>,
        failure: crate::gateway::session_lease::LeaseCheckFailure,
    ) -> Self {
        match failure {
            crate::gateway::session_lease::LeaseCheckFailure::Busy {
                holder_client_id,
                holder_hostname,
                holder_pid,
                last_heartbeat_ms,
                stale,
            } => Self::SessionAttachedByAnotherClient {
                session_id: session_id.into(),
                holder_client_id,
                holder_hostname: holder_hostname.unwrap_or_default(),
                holder_pid: holder_pid.unwrap_or(0),
                last_heartbeat_ms,
                stale,
            },
            crate::gateway::session_lease::LeaseCheckFailure::ClockSkew => Self::LeaseClockSkew {
                session_id: session_id.into(),
            },
        }
    }
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
        _cancellation_token: Option<CancellationToken>,
        _tool_event_sink: Option<Arc<dyn agent_contracts::ToolEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_with_events(request, event_sink).await
    }

    async fn export_session(
        &self,
        _session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "export_session".to_string(),
        })
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

    async fn force_close_session(
        &self,
        _session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        Err(SessionServiceError::UnsupportedCapability {
            capability: "force_close_session".to_string(),
        })
    }
}
