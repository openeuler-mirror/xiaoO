use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationBackendConfig {
    pub kind: String,
    pub options: Value,
}

impl OperationBackendConfig {
    pub fn new(kind: impl Into<String>, options: Value) -> Self {
        Self {
            kind: kind.into(),
            options,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OperationBackendBuildInput {
    pub config: Option<OperationBackendConfig>,
    pub workspace_root: Option<PathBuf>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub sender_id: Option<String>,
    pub channel: Option<String>,
    pub channel_instance_id: Option<String>,
}

#[allow(dead_code)]
fn default_backend_options() -> Value {
    Value::Object(serde_json::Map::new())
}
