use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}
