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
    #[serde(skip)]
    #[allow(dead_code)]
    pub extra_fields: Option<serde_json::Value>,
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

// impl WireRequest {
//     pub(crate) fn new(model: String, messages: Vec<WireMessage>) -> Self {
//         Self {
//             model,
//             messages,
//             temperature: None,
//             max_tokens: None,
//             stream: None,
//             tools: None,
//             tool_choice: None,
//             response_format: None,
//             route_info: None,
//             extra_fields: None,
//         }
//     }

//     pub(crate) fn with_temperature(mut self, temp: f32) -> Self {
//         self.temperature = Some(Temperature::new(temp));
//         self
//     }

//     pub(crate) fn with_max_tokens(mut self, tokens: u32) -> Self {
//         self.max_tokens = Some(tokens);
//         self
//     }

//     pub(crate) fn with_stream(mut self, stream: bool) -> Self {
//         self.stream = Some(stream);
//         self
//     }

//     pub(crate) fn with_tools(mut self, tools: Vec<WireTool>) -> Self {
//         self.tools = Some(tools);
//         self
//     }

//     pub(crate) fn with_tool_choice(mut self, tool_choice: WireToolChoice) -> Self {
//         self.tool_choice = Some(tool_choice);
//         self
//     }

//     pub(crate) fn with_response_format(mut self, response_format: WireResponseFormat) -> Self {
//         self.response_format = Some(response_format);
//         self
//     }

//     pub(crate) fn is_streaming(&self) -> bool {
//         self.stream.unwrap_or(false)
//     }
// }
