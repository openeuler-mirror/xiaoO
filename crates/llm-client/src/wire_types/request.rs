use serde::{Deserialize, Serialize};

use super::format::WireResponseFormat;
use super::message::WireMessage;
use super::route_info::RouteInfo;
use super::temperature::Temperature;
use super::tool::{WireTool, WireToolChoice};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WireRequest {
    pub model: String,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Temperature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<WireToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<WireResponseFormat>,
    #[serde(skip)]
    pub route_info: Option<RouteInfo>,
}

impl std::fmt::Debug for WireRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireRequest")
            .field("model", &self.model)
            .field("messages", &self.messages)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("stream", &self.stream)
            .field("tools", &self.tools.as_ref().map(|_| "<tools>"))
            .field("tool_choice", &self.tool_choice)
            .field(
                "response_format",
                &self.response_format.as_ref().map(|_| "<response_format>"),
            )
            .field("route_info", &self.route_info)
            .finish()
    }
}
