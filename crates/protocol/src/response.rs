//! Daemon handshake and HTTP response contracts used by external clients.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
