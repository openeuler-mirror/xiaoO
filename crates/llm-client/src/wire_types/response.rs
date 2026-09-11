use serde::{Deserialize, Serialize};

use super::message::WireMessage;
use super::tool::WireToolCall;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Warning {
    pub feature: String,
    pub provider: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[allow(dead_code)]
impl Warning {
    pub(crate) fn new(
        feature: impl Into<String>,
        provider: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            feature: feature.into(),
            provider: provider.into(),
            action: action.into(),
            message: None,
        }
    }
    #[allow(dead_code)]
    pub(crate) fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KvTransferParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunk_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<WireChoice>,
    pub usage: WireUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<Warning>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_transfer_params: Option<KvTransferParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireChoice {
    pub message: WireMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub prompt_tokens_details: Option<WirePromptTokensDetails>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct WirePromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/wire_types/response_test.rs"]
mod tests;
