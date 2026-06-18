use agent_contracts::backend::{
    BackendControlError, BackendEndpoint, BackendInstance, BackendLifecycleState,
    BackendResourceAllocation, BackendResourceLimits, OperationBackend, OperationBackendBuildError,
    OperationError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_MAX_ACTIVE_E2B_SANDBOXES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayBackendConfig {
    pub kind: String,
    pub options: Value,
}

impl GatewayBackendConfig {
    pub fn new(kind: impl Into<String>, options: Value) -> Self {
        Self {
            kind: kind.into(),
            options,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendManagerLimits {
    pub max_active_e2b_sandboxes: Option<usize>,
}

impl Default for BackendManagerLimits {
    fn default() -> Self {
        Self {
            max_active_e2b_sandboxes: Some(DEFAULT_MAX_ACTIVE_E2B_SANDBOXES),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackendEnsureSessionRequest {
    pub config: Option<GatewayBackendConfig>,
    pub workspace_root: PathBuf,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCreateRequest {
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub backend_id: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub resource_limits: BackendResourceLimits,
    #[serde(default)]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendConnectRequest {
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BackendListFilter {
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendLineageInfo {
    #[serde(default)]
    pub parent_backend_id: Option<String>,
    #[serde(default)]
    pub children_backend_ids: Vec<String>,
    #[serde(default)]
    pub forked_from_snapshot_id: Option<String>,
    #[serde(default)]
    pub forked_snapshot_names: Vec<String>,
    #[serde(default)]
    pub forked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub backend_id: String,
    pub provider: String,
    pub instance_id: String,
    pub state: BackendLifecycleState,
    pub workspace_root: String,
    pub endpoint: Option<BackendEndpoint>,
    pub metadata: Value,
    pub resources: BackendResourceAllocation,
    pub session_id: Option<String>,
    #[serde(default)]
    pub session_ids: Vec<String>,
    pub expires_at_ms: Option<u64>,
    #[serde(default)]
    pub lineage: BackendLineageInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendForkRequest {
    #[serde(default)]
    pub parent_backend_id: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub backend_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub resource_limits: BackendResourceLimits,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub snapshot_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCheckpointRef {
    pub checkpoint_id: String,
    pub provider: String,
    #[serde(default)]
    pub source_backend_id: Option<String>,
    #[serde(default)]
    pub provider_snapshot_id: Option<String>,
    #[serde(default)]
    pub provider_snapshot_names: Vec<String>,
    pub workspace_root: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub metadata: Value,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing)]
    pub provider_options: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendCheckpointRequest {
    #[serde(default)]
    pub backend_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCheckpointResult {
    pub backend: BackendInfo,
    pub checkpoint: BackendCheckpointRef,
    pub reused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCheckpointSnapshotDeleteRequest {
    pub checkpoint: BackendCheckpointRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCheckpointSnapshotDeleteResult {
    pub checkpoint_id: String,
    pub provider: String,
    #[serde(default)]
    pub provider_snapshot_id: Option<String>,
    #[serde(default)]
    pub provider_snapshot_names: Vec<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCheckoutRequest {
    pub checkpoint: BackendCheckpointRef,
    #[serde(default)]
    pub backend_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub resource_limits: BackendResourceLimits,
    #[serde(default)]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCheckoutResult {
    pub backend: BackendInfo,
    pub checkpoint: BackendCheckpointRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendForkResult {
    pub parent: BackendInfo,
    pub child: BackendInfo,
    pub snapshot_id: String,
    #[serde(default)]
    pub snapshot_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendTreeNode {
    pub backend: BackendInfo,
    #[serde(default)]
    pub children: Vec<BackendTreeNode>,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("invalid managed backend request: {message}")]
    InvalidRequest { message: String },
    #[error("managed backend conflict: {message}")]
    Conflict { message: String },
    #[error("managed backend not found: {backend_id}")]
    NotFound { backend_id: String },
    #[error("unsupported backend kind: {kind}")]
    UnsupportedBackend { kind: String },
    #[error("managed backend resource limit exceeded: {message}")]
    ResourceLimitExceeded { message: String },
    #[error("managed backend build failed: {message}")]
    BuildFailed { message: String },
    #[error("managed backend operation failed: {message}")]
    Operation { message: String },
}

impl BackendError {
    pub(super) fn from_build_error(error: OperationBackendBuildError) -> Self {
        match error {
            OperationBackendBuildError::InvalidConfig { message } => {
                Self::InvalidRequest { message }
            }
            OperationBackendBuildError::UnsupportedBackend { kind } => {
                Self::UnsupportedBackend { kind }
            }
            OperationBackendBuildError::ResourceLimitExceeded { message } => {
                Self::ResourceLimitExceeded { message }
            }
            OperationBackendBuildError::Unsupported { message }
            | OperationBackendBuildError::BuildFailed { message } => Self::BuildFailed { message },
        }
    }

    pub(super) fn from_control_error(error: BackendControlError) -> Self {
        match error {
            BackendControlError::InvalidRequest { message } => Self::InvalidRequest { message },
            BackendControlError::UnsupportedCapability {
                provider,
                capability,
            } => Self::UnsupportedBackend {
                kind: format!("{}:{capability}", provider.0),
            },
            BackendControlError::NotFound { id, .. } => Self::NotFound { backend_id: id },
            BackendControlError::InvalidState { message, .. } => Self::Conflict { message },
            BackendControlError::ProviderError { message, .. }
            | BackendControlError::Transport { message } => Self::BuildFailed { message },
            BackendControlError::Timeout {
                operation,
                timeout_ms,
            } => Self::BuildFailed {
                message: format!("{operation} timed out after {timeout_ms} ms"),
            },
        }
    }

    pub(super) fn from_operation_error(error: OperationError) -> Self {
        Self::Operation {
            message: error.to_string(),
        }
    }

    pub(super) fn into_build_error(self) -> OperationBackendBuildError {
        match self {
            Self::InvalidRequest { message } | Self::Conflict { message } => {
                OperationBackendBuildError::InvalidConfig { message }
            }
            Self::UnsupportedBackend { kind } => {
                OperationBackendBuildError::UnsupportedBackend { kind }
            }
            Self::ResourceLimitExceeded { message } => {
                OperationBackendBuildError::ResourceLimitExceeded { message }
            }
            Self::NotFound { backend_id } => OperationBackendBuildError::BuildFailed {
                message: format!("managed backend not found: {backend_id}"),
            },
            Self::BuildFailed { message } | Self::Operation { message } => {
                OperationBackendBuildError::BuildFailed { message }
            }
        }
    }

    pub(super) fn into_operation_error(self) -> OperationError {
        OperationError::Transport {
            message: self.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct BackendLease {
    backend: Arc<dyn OperationBackend>,
    instance: BackendInstance,
}

impl BackendLease {
    pub(super) fn new(backend: Arc<dyn OperationBackend>, instance: BackendInstance) -> Self {
        Self { backend, instance }
    }

    pub fn backend(&self) -> Arc<dyn OperationBackend> {
        Arc::clone(&self.backend)
    }

    pub fn instance(&self) -> BackendInstance {
        self.instance.clone()
    }
}
