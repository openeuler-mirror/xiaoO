use super::storage::StorageConfig;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub transport: CheckpointTransport,
    #[serde(default)]
    pub unix_socket: Option<UnixSocketConfig>,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub save_mode: SaveMode,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTransport {
    #[default]
    UnixSocket,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnixSocketConfig {
    pub endpoint: std::path::PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMode {
    /// Capture when the queued save executes (existing behavior).
    #[default]
    Next,
    /// Pin the most recently published boundary when save is accepted.
    Current,
}

/// Per-binding overrides. Omitted values fall back to startup configuration,
/// including on re-init after release.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckpointInitOptions {
    #[serde(default)]
    pub save_mode: Option<SaveMode>,
}
