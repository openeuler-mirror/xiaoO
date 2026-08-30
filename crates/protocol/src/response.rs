//! Daemon handshake and HTTP response contracts used by external clients.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Readiness message discriminator.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DaemonReadyMessageType {
    /// Daemon completed startup.
    Ready,
}

/// Stable daemon service identifier.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum DaemonService {
    /// xiaoO daemon service.
    #[serde(rename = "xiaoo-daemon")]
    XiaooDaemon,
}

/// Health response status.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GatewayHealthStatus {
    /// Service is healthy.
    Ok,
}

/// Runtime transport identifier.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum GatewayTransport {
    /// HTTP requests with SSE streaming responses.
    #[serde(rename = "http+sse")]
    HttpSse,
}

/// Managed-daemon readiness message written to stdout.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DaemonReadyMessage {
    /// Message discriminator.
    pub r#type: DaemonReadyMessageType,
    /// Service identifier.
    pub service: DaemonService,
    /// Bound host address.
    pub host: String,
    /// Bound TCP port.
    pub port: u16,
    /// Daemon package version.
    pub version: String,
    /// Runtime wire protocol version.
    pub protocol_version: u32,
}

/// Response returned by `/api/v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GatewayHealthResponse {
    /// Health state.
    pub status: GatewayHealthStatus,
    /// Daemon package version.
    pub version: String,
}

/// Feature flags returned by `/api/v1/capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GatewayFeatureCapabilities {
    /// Whether sessions survive daemon restarts.
    pub session_persistence: bool,
    /// Whether runtimes enforce single-writer leases.
    pub runtime_leases: bool,
    /// Whether runtime checkpoints are available.
    pub checkpoints: bool,
    /// Whether runtime file operations are available.
    pub file_operations: bool,
    /// Whether file-change summaries are emitted.
    pub file_change_summary: bool,
    /// Whether file-change patches are emitted.
    pub file_change_patch: bool,
    /// Whether SSE streams can resume from an event cursor.
    pub sse_resume: bool,
}

/// Management capabilities for one functional domain.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ManagementDomainCapability {
    /// Stable domain identifier.
    pub id: String,
    /// Whether the domain has configuration fields.
    pub configurable: bool,
    /// Whether runtime state can be read.
    pub runtime_read: bool,
    /// Whether runtime state can be changed.
    pub runtime_write: bool,
    /// Whether the domain exposes a test operation.
    pub test: bool,
    /// Supported action identifiers.
    pub actions: Vec<String>,
}

/// Configuration and runtime management capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ManagementCapabilities {
    /// Management API version.
    pub api_version: u32,
    /// Configuration Schema version.
    pub config_schema_version: u32,
    /// Available pre-start configuration commands.
    pub config_commands: Vec<String>,
    /// Per-domain management capabilities.
    pub domains: Vec<ManagementDomainCapability>,
}

/// Response returned by `/api/v1/capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GatewayCapabilitiesResponse {
    /// Service identifier.
    pub service: DaemonService,
    /// Daemon package version.
    pub version: String,
    /// Current wire protocol version.
    pub protocol_version: u32,
    /// Minimum client wire protocol version.
    pub minimum_client_protocol_version: u32,
    /// Runtime transport identifier.
    pub transport: GatewayTransport,
    /// Supported Runtime API actions.
    pub runtime_api: Vec<String>,
    /// Supported SSE event names.
    pub sse_events: Vec<String>,
    /// Supported interaction kinds.
    pub interaction_kinds: Vec<String>,
    /// Runtime feature flags.
    pub features: GatewayFeatureCapabilities,
    /// Configuration and management capabilities.
    pub management: ManagementCapabilities,
}

/// Standard JSON error response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GatewayErrorResponse {
    /// Human-readable error detail.
    pub error: String,
}

/// Minimum Runtime snapshot required by clients after opening a Runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeOpenSnapshot {
    /// Dynamic primary Agent identifier.
    pub agent_id: String,
}

/// Stable client-facing subset of the Runtime open response.
///
/// The daemon may return additional Runtime state, but clients can rely on
/// these fields without depending on service-internal snapshot structures.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeOpenResponse {
    /// Opened Runtime identifier.
    pub runtime_id: String,
    /// Runtime metadata required to route streamed Agent events.
    pub runtime: RuntimeOpenSnapshot,
}

/// Runtime lifecycle state exposed by management responses.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLifecycleStatus {
    /// Runtime is ready for input.
    Idle,
    /// Runtime is processing a turn.
    Running,
    /// Runtime is paused and its backend has been released.
    Paused,
    /// Runtime stopped because of a failure.
    Failed,
    /// Runtime was closed.
    Closed,
}

/// Stable Runtime metadata returned by management operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeRecordResponse {
    /// Runtime identifier.
    pub runtime_id: String,
    /// Conversation identifier.
    pub conversation_id: String,
    /// Sender identity associated with the Runtime.
    pub sender_id: String,
    /// Current lifecycle state.
    pub status: RuntimeLifecycleStatus,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
    /// Last update time in Unix milliseconds.
    pub updated_at_ms: u64,
}

/// Response returned after creating a Runtime checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeCheckpointResponse {
    /// Created checkpoint identifier.
    pub checkpoint_id: String,
    /// Runtime metadata at checkpoint creation.
    pub runtime: RuntimeRecordResponse,
    /// Parent checkpoint identifier, when present.
    pub parent_checkpoint_id: Option<String>,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
    /// Caller-provided checkpoint metadata.
    pub metadata: Value,
    /// Optional checkpoint display name.
    pub name: Option<String>,
}

/// Response returned after checking out a Runtime checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeCheckoutResponse {
    /// Source checkpoint identifier.
    pub checkpoint_id: String,
    /// Runtime identifier from which the checkpoint was created.
    pub source_runtime_id: String,
    /// Newly created Runtime metadata.
    pub runtime: RuntimeRecordResponse,
}

/// Response returned after pausing a Runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimePauseResponse {
    /// Paused Runtime metadata.
    pub runtime: RuntimeRecordResponse,
    /// Checkpoint created for the paused Runtime.
    pub checkpoint_id: String,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
    /// Caller-provided pause metadata.
    pub metadata: Value,
    /// Optional checkpoint display name.
    pub name: Option<String>,
}

/// Response returned after resuming a Runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeResumeResponse {
    /// Resumed Runtime metadata.
    pub runtime: RuntimeRecordResponse,
}

/// Response returned after deleting a provider checkpoint snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeCheckpointSnapshotDeleteResponse {
    /// Checkpoint identifier.
    pub checkpoint_id: String,
    /// Runtime identifier associated with the checkpoint.
    pub runtime_id: String,
    /// Backend provider name, when a provider snapshot existed.
    pub provider: Option<String>,
    /// Provider snapshot identifier, when available.
    pub provider_snapshot_id: Option<String>,
    /// Provider snapshot names associated with the checkpoint.
    pub provider_snapshot_names: Vec<String>,
    /// Whether a provider snapshot was deleted.
    pub deleted_provider_snapshot: bool,
    /// Deletion time in Unix milliseconds.
    pub deleted_at_ms: u64,
}

/// Response returned after executing a command in a Runtime backend.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeExecResponse {
    /// Base64-encoded standard output.
    pub stdout_base64: String,
    /// Base64-encoded standard error.
    pub stderr_base64: String,
    /// Process exit code, when the process started and exited normally.
    pub exit_code: Option<i32>,
    /// Whether execution exceeded its timeout.
    pub timed_out: bool,
}

/// Error response returned when Runtime execution is interrupted.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeExecInterruptedResponse {
    /// Human-readable execution error.
    pub error: String,
    /// Backend execution state at interruption.
    pub execution_state: String,
    /// Base64-encoded partial standard output.
    pub stdout_base64: String,
    /// Base64-encoded partial standard error.
    pub stderr_base64: String,
    /// Whether retrying the command is safe.
    pub retryable: bool,
}

/// Response returned after reading a Runtime backend file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeReadFileResponse {
    /// Base64-encoded file content.
    pub content_base64: String,
}

/// Response returned after writing a Runtime backend file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeWriteFileResponse {
    /// Written file path.
    pub path: String,
    /// Whether the operation created a new file.
    pub created: bool,
}
