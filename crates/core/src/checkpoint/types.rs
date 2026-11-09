use super::CheckpointDriver;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindingInfo {
    pub session_id: String,
}
pub struct CheckpointBinding {
    pub info: BindingInfo,
    /// Present only on the first init. Attach it to the agent and retain it
    /// across release/init; subsequent init only queues activation.
    pub driver: Option<CheckpointDriver>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub session_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotId(pub String);

/// Session-local display suffix; the UUID remains the unique identity.
#[derive(Default)]
pub(super) struct SnapshotIdGenerator {
    next_suffix: u8,
}

impl SnapshotIdGenerator {
    pub fn generate(&mut self) -> SnapshotId {
        let suffix = char::from(b'a' + self.next_suffix);
        self.next_suffix = (self.next_suffix + 1) % 26;
        SnapshotId(format!("ctxcp_{}_{suffix}", uuid::Uuid::new_v4().simple()))
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    EnsureSafeMode,
    PromiseMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub snapshot_id: SnapshotId,
    /// Required: there is deliberately no default restore mode.
    pub mode: RestoreMode,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("invalid checkpoint init config: {0}")]
    InvalidConfig(String),
    #[error("Not implemented: {0}")]
    NotImplemented(&'static str),
    #[error("no current checkpoint is available yet")]
    NoCurrentCheckpoint,
    #[error("checkpoint backend is not initialized")]
    NotInitialized,
    #[error("checkpoint backend is already initialized")]
    AlreadyInitialized,
    #[error("checkpoint binding has been released")]
    Released,
    #[error("snapshot is not ready or does not exist: {0:?}")]
    SnapshotNotReadyOrNotFound(SnapshotId),
    #[error("snapshot creation failed: {0:?}")]
    SnapshotFailed(SnapshotId),
    #[error("checkpoint belongs to a different session")]
    SessionMismatch,
    #[error("checkpoint queue is closed")]
    QueueClosed,
    #[error("checkpoint storage: {0}")]
    Storage(String),
    #[error("checkpoint socket: {0}")]
    Io(#[from] std::io::Error),
}

/// Shared session context for checkpoint operations.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotContext {
    /// Empty during socket init means the endpoint's current session.
    #[serde(default)]
    pub session_id: String,
}

impl SnapshotContext {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    LastN,
}
