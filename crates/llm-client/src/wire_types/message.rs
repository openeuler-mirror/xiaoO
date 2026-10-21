use serde::{Deserialize, Serialize};

use super::tool::WireToolCall;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/wire_types/message_test.rs"]
mod tests;
