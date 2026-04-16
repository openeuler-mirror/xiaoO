use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolLifecycleEvent {
    Pending {
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args_preview: String,
    },
    Running {
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args_preview: String,
    },
    Denied {
        call_id: String,
        tool_name: String,
        reason: String,
        #[serde(default)]
        args_preview: String,
    },
    Completed {
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args_preview: String,
    },
    Failed {
        call_id: String,
        tool_name: String,
        error: String,
        #[serde(default)]
        args_preview: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultEvent {
    pub call_id: String,
    pub tool_name: String,
    pub output_preview: String,
    pub is_error: bool,
    #[serde(default)]
    pub args_preview: String,
}
