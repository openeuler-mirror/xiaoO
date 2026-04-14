use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolLifecycleEvent {
    Pending {
        call_id: String,
        tool_name: String,
    },
    Running {
        call_id: String,
        tool_name: String,
    },
    Denied {
        call_id: String,
        tool_name: String,
        reason: String,
    },
    Completed {
        call_id: String,
        tool_name: String,
    },
    Failed {
        call_id: String,
        tool_name: String,
        error: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultEvent {
    pub call_id: String,
    pub tool_name: String,
    pub output_preview: String,
    pub is_error: bool,
}
