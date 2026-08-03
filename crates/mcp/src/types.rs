use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- JSON-RPC 2.0 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Id,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub data: Option<Value>,
}

/// JSON-RPC id. MCP servers use numeric ids; we emit u64.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum Id {
    Num(u64),
    Str(String),
}

impl From<u64> for Id {
    fn from(v: u64) -> Self {
        Self::Num(v)
    }
}

impl Id {
    /// Resolve to a numeric id used to key pending requests.
    ///
    /// This client only emits numeric ids, so a non-numeric string id cannot
    /// correspond to any pending request. Returns `None` in that case so the
    /// caller can drop the response instead of silently misrouting it (e.g. to
    /// a real pending request with id `0`).
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Id::Num(n) => Some(*n),
            Id::Str(s) => s.parse().ok(),
        }
    }
}

/// A raw inbound message: either a response matching a pending request, or a
/// server-initiated notification.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum InboundMessage {
    Response(JsonRpcResponse),
    #[allow(dead_code)]
    Notification(JsonRpcNotification),
}

// ---------- MCP protocol payloads ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: Value,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default)]
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// A discovered MCP tool: name, description, and JSON-Schema input schema.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolParams {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub structured_content: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        #[serde(default, rename = "data")]
        data: String,
        #[serde(default, rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        #[serde(default)]
        resource: ResourceRef,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRef {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}
