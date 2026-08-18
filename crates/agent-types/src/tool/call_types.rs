use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FinalToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    /// Per-invocation plugin contributions (hooker_id + payload), accumulated
    /// by pre-hooks and passed to the execution backend via `ExecRequest.extra`.
    /// Never serialized when empty so the `extra` key is absent from any JSON
    /// the plugin hookers observe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}
