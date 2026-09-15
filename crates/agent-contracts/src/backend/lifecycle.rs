use super::{BackendPath, OperationBackend};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackendId(pub String);

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackendProviderKind(pub String);

impl fmt::Display for BackendProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackendInstanceId(pub String);

impl fmt::Display for BackendInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackendSnapshotId(pub String);

impl fmt::Display for BackendSnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendLifecycleState {
    Unknown,
    Creating,
    Active,
    Pausing,
    Paused,
    Loading,
    Deleting,
    Deleted,
    Failed,
}

impl BackendLifecycleState {
    pub fn operation_plane_usable(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Deleted)
    }

    pub fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Creating | Self::Pausing | Self::Loading | Self::Deleting
        )
    }
}

impl fmt::Display for BackendLifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Unknown => "unknown",
            Self::Creating => "creating",
            Self::Active => "active",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Loading => "loading",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
            Self::Failed => "failed",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendLifecycleOperation {
    CreateSandbox,
    Load,
    Pause,
    Delete,
    Inspect,
}

impl BackendLifecycleOperation {
    pub fn in_progress_state(self) -> Option<BackendLifecycleState> {
        match self {
            Self::CreateSandbox => Some(BackendLifecycleState::Creating),
            Self::Load => Some(BackendLifecycleState::Loading),
            Self::Pause => Some(BackendLifecycleState::Pausing),
            Self::Delete => Some(BackendLifecycleState::Deleting),
            Self::Inspect => None,
        }
    }

    pub fn success_state(self) -> Option<BackendLifecycleState> {
        match self {
            Self::CreateSandbox | Self::Load => Some(BackendLifecycleState::Active),
            Self::Pause => Some(BackendLifecycleState::Paused),
            Self::Delete => Some(BackendLifecycleState::Deleted),
            Self::Inspect => None,
        }
    }
}

impl fmt::Display for BackendLifecycleOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::CreateSandbox => "create_sandbox",
            Self::Load => "load",
            Self::Pause => "pause",
            Self::Delete => "delete",
            Self::Inspect => "inspect",
        };
        f.write_str(value)
    }
}

pub struct BackendLifecycleStateMachine;

impl BackendLifecycleStateMachine {
    pub fn begin(
        current: Option<BackendLifecycleState>,
        operation: BackendLifecycleOperation,
    ) -> Result<BackendLifecycleState, BackendControlError> {
        if matches!(operation, BackendLifecycleOperation::Inspect) {
            return Ok(current.unwrap_or(BackendLifecycleState::Unknown));
        }

        let allowed = match (current, operation) {
            (None, BackendLifecycleOperation::CreateSandbox) => true,
            (None, BackendLifecycleOperation::Load) => true,
            (Some(BackendLifecycleState::Paused), BackendLifecycleOperation::Load) => true,
            (Some(BackendLifecycleState::Active), BackendLifecycleOperation::Pause) => true,
            (Some(BackendLifecycleState::Active), BackendLifecycleOperation::Delete) => true,
            (Some(BackendLifecycleState::Paused), BackendLifecycleOperation::Delete) => true,
            (Some(BackendLifecycleState::Failed), BackendLifecycleOperation::Delete) => true,
            (Some(BackendLifecycleState::Deleted), BackendLifecycleOperation::Delete) => {
                return Ok(BackendLifecycleState::Deleted);
            }
            _ => false,
        };

        if !allowed {
            return Err(BackendControlError::InvalidState {
                current,
                requested_operation: operation,
                message: "backend lifecycle operation is not valid for current state".to_string(),
            });
        }

        operation
            .in_progress_state()
            .ok_or_else(|| BackendControlError::InvalidState {
                current,
                requested_operation: operation,
                message: "operation has no in-progress state".to_string(),
            })
    }

    pub fn complete_success(
        current: BackendLifecycleState,
        operation: BackendLifecycleOperation,
    ) -> Result<BackendLifecycleState, BackendControlError> {
        let Some(expected_current) = operation.in_progress_state() else {
            return Err(BackendControlError::InvalidState {
                current: Some(current),
                requested_operation: operation,
                message: "inspect completion must use reconcile_inspected_state".to_string(),
            });
        };

        if current == BackendLifecycleState::Deleted
            && operation == BackendLifecycleOperation::Delete
        {
            return Ok(BackendLifecycleState::Deleted);
        }

        if current != expected_current {
            return Err(BackendControlError::InvalidState {
                current: Some(current),
                requested_operation: operation,
                message: format!("expected state {expected_current} before completing operation"),
            });
        }

        operation
            .success_state()
            .ok_or_else(|| BackendControlError::InvalidState {
                current: Some(current),
                requested_operation: operation,
                message: "operation has no success state".to_string(),
            })
    }

    pub fn complete_failure(
        current: BackendLifecycleState,
        operation: BackendLifecycleOperation,
    ) -> Result<BackendLifecycleState, BackendControlError> {
        if matches!(operation, BackendLifecycleOperation::Inspect) {
            return Ok(current);
        }

        let Some(expected_current) = operation.in_progress_state() else {
            return Ok(BackendLifecycleState::Failed);
        };

        if current == BackendLifecycleState::Deleted
            && operation == BackendLifecycleOperation::Delete
        {
            return Ok(BackendLifecycleState::Deleted);
        }

        if current != expected_current {
            return Err(BackendControlError::InvalidState {
                current: Some(current),
                requested_operation: operation,
                message: format!("expected state {expected_current} before failing operation"),
            });
        }

        Ok(BackendLifecycleState::Failed)
    }

    pub fn reconcile_inspected_state(
        inspected: BackendLifecycleState,
    ) -> Result<BackendLifecycleState, BackendControlError> {
        Ok(inspected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendEndpoint {
    Local,
    Tcp { host: String, port: u16 },
    UnixSocket { path: String },
    ProviderHandle { value: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendSnapshotRef {
    pub snapshot_id: BackendSnapshotId,
    pub provider: BackendProviderKind,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BackendRuntimeCapabilities {
    pub supports_exec: bool,
    pub supports_file_read: bool,
    pub supports_file_write: bool,
    pub supports_search: bool,
    pub supports_export_file: bool,
    pub supports_lsp: bool,
    pub supports_pause: bool,
    pub supports_snapshot: bool,
    pub supports_delete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BackendResourceLimits {
    pub vcpu_count: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BackendResourceAllocation {
    pub vcpu_count: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendInstance {
    pub backend_id: BackendId,
    pub provider: BackendProviderKind,
    pub instance_id: BackendInstanceId,
    pub session_id: String,
    pub state: BackendLifecycleState,
    pub workspace_root: BackendPath,
    pub endpoint: Option<BackendEndpoint>,
    pub snapshot: Option<BackendSnapshotRef>,
    pub capabilities: BackendRuntimeCapabilities,
    pub resources: BackendResourceAllocation,
    pub metadata: Value,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendInstanceStatus {
    pub backend_id: BackendId,
    pub provider: BackendProviderKind,
    pub instance_id: Option<BackendInstanceId>,
    pub state: BackendLifecycleState,
    pub endpoint: Option<BackendEndpoint>,
    pub snapshot: Option<BackendSnapshotRef>,
    pub last_error: Option<String>,
    pub metadata: Value,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendSnapshot {
    pub snapshot_id: BackendSnapshotId,
    pub provider: BackendProviderKind,
    pub source_backend_id: BackendId,
    pub source_instance_id: BackendInstanceId,
    pub state: BackendLifecycleState,
    pub metadata: Value,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCreateRequest {
    pub requested_backend_id: Option<BackendId>,
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub workspace_root: BackendPath,
    pub provider_options: Value,
    pub resource_limits: BackendResourceLimits,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendLoadRequest {
    pub session_id: String,
    pub workspace_root: BackendPath,
    pub source: BackendLoadSource,
    pub provider_options: Value,
    pub resource_limits: BackendResourceLimits,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendLoadSource {
    SnapshotId(BackendSnapshotId),
    InstanceId(BackendInstanceId),
    SerializedHandle(Value),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendPauseRequest {
    pub backend_id: BackendId,
    pub instance_id: BackendInstanceId,
    pub mode: BackendPauseMode,
    pub reason: BackendLifecycleReason,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendPauseMode {
    SnapshotAndStop,
    FreezeInPlace,
    BestEffort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDeleteRequest {
    pub backend_id: BackendId,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub force: bool,
    pub reason: BackendLifecycleReason,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendInspectRequest {
    pub backend_id: Option<BackendId>,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub metadata: Value,
}

impl BackendInspectRequest {
    pub fn has_target(&self) -> bool {
        self.backend_id.is_some() || self.instance_id.is_some() || self.snapshot_id.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendLifecycleReason {
    SessionOpen,
    SessionResume,
    SessionIdle,
    SessionClose,
    DaemonShutdown,
    UserRequested,
    ErrorCleanup,
    ManagerReconcile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDeleteOutcome {
    pub backend_id: BackendId,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub state: BackendLifecycleState,
    pub already_deleted: bool,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendManagerRecord {
    pub backend_id: BackendId,
    pub provider: BackendProviderKind,
    pub session_id: String,
    pub instance_id: Option<BackendInstanceId>,
    pub snapshot_id: Option<BackendSnapshotId>,
    pub state: BackendLifecycleState,
    pub workspace_root: BackendPath,
    pub endpoint: Option<BackendEndpoint>,
    pub capabilities: BackendRuntimeCapabilities,
    pub metadata: Value,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendControlError {
    #[error("invalid backend lifecycle request: {message}")]
    InvalidRequest { message: String },

    #[error(
        "invalid backend lifecycle state for {requested_operation}: current={current:?}: {message}"
    )]
    InvalidState {
        current: Option<BackendLifecycleState>,
        requested_operation: BackendLifecycleOperation,
        message: String,
    },

    #[error("backend provider '{provider}' does not support capability '{capability}'")]
    UnsupportedCapability {
        provider: BackendProviderKind,
        capability: String,
    },

    #[error("backend provider '{provider}' could not find id '{id}'")]
    NotFound {
        provider: BackendProviderKind,
        id: String,
    },

    #[error("backend provider '{provider}' failed: {message}")]
    ProviderError {
        provider: BackendProviderKind,
        message: String,
    },

    #[error("backend lifecycle operation '{operation}' timed out after {timeout_ms} ms")]
    Timeout {
        operation: BackendLifecycleOperation,
        timeout_ms: u64,
    },

    #[error("backend lifecycle transport error: {message}")]
    Transport { message: String },
}

#[async_trait]
pub trait BackendLifecycle: Send + Sync {
    async fn create_sandbox(
        &self,
        request: BackendCreateRequest,
    ) -> Result<BackendInstance, BackendControlError>;

    async fn load(
        &self,
        request: BackendLoadRequest,
    ) -> Result<BackendInstance, BackendControlError>;

    async fn pause(
        &self,
        request: BackendPauseRequest,
    ) -> Result<BackendSnapshot, BackendControlError>;

    async fn delete(
        &self,
        request: BackendDeleteRequest,
    ) -> Result<BackendDeleteOutcome, BackendControlError>;

    async fn inspect(
        &self,
        request: BackendInspectRequest,
    ) -> Result<BackendInstanceStatus, BackendControlError>;
}

#[async_trait]
pub trait BackendProvider: Send + Sync {
    fn kind(&self) -> BackendProviderKind;

    fn lifecycle(&self) -> Arc<dyn BackendLifecycle>;

    async fn attach(
        &self,
        instance: BackendInstance,
    ) -> Result<Arc<dyn OperationBackend>, BackendControlError>;
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-contracts/backend/lifecycle_test.rs"]
mod tests;
