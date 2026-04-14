use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolLifecycleRecord {
    pub call_id: String,
    pub tool_name: String,
    pub status: ToolLifecycleStatus,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLifecycleStatus {
    Pending,
    Running,
    Denied,
    Completed,
    Suspended,
    Failed,
}
