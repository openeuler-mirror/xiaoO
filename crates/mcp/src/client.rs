use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::config::{McpServerConfig, Transport};
use crate::error::McpError;
use crate::transport::{McpTransport, SseTransport, StdioTransport, StreamableHttpTransport};
use crate::types::{
    CallToolParams, CallToolResult, ClientInfo, ContentBlock, InitializeParams, InitializeResult,
    ListToolsResult, McpToolDef,
};

/// Result of invoking an MCP tool.
#[derive(Debug, Clone)]
pub struct McpCallResult {
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub structured_content: Option<Value>,
}

/// A connected, initialised MCP client. Cloning shares the underlying
/// transport; resources (child process / HTTP stream) are released when the
/// last clone is dropped.
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    server_name: String,
    next_id: AtomicU64,
}

impl McpClient {
    /// Connect to the server described by `config` (does not perform the
    /// protocol handshake — call `initialize` next).
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        let transport: Arc<dyn McpTransport> = match config.transport {
            Transport::Stdio => {
                let command = config.command.clone().ok_or_else(|| {
                    McpError::Protocol("stdio transport requires `command`".to_string())
                })?;
                Arc::new(
                    StdioTransport::spawn(&command, &config.args, &config.env, config.timeout_ms)
                        .await?,
                )
            }
            Transport::Sse => {
                let url = config.url.clone().ok_or_else(|| {
                    McpError::Protocol("sse transport requires `url`".to_string())
                })?;
                Arc::new(SseTransport::connect(&url, config.timeout_ms).await?)
            }
            Transport::StreamableHttp => Arc::new(StreamableHttpTransport::connect(config).await?),
        };

        Ok(Self {
            transport,
            server_name: config.name.clone(),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Perform the MCP `initialize` handshake.
    pub async fn initialize(&self) -> Result<InitializeResult, McpError> {
        let params = InitializeParams {
            protocol_version: self.transport.initialize_protocol_version().to_string(),
            capabilities: serde_json::json!({}),
            client_info: ClientInfo {
                name: "xiaoo".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        let params_value =
            serde_json::to_value(&params).map_err(|e| McpError::Protocol(e.to_string()))?;
        let result = self
            .transport
            .send_request(self.next_id(), "initialize", Some(params_value))
            .await?;
        let init: InitializeResult =
            serde_json::from_value(result).map_err(|e| McpError::HandshakeFailed(e.to_string()))?;
        self.transport
            .validate_negotiated_protocol_version(&init.protocol_version)?;
        self.transport
            .set_protocol_version(&init.protocol_version)
            .await;
        // Notify the server that initialisation is complete.
        self.transport
            .send_notification("notifications/initialized", None)
            .await?;
        Ok(init)
    }

    /// List all tools exposed by the server, following pagination cursors.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<Value> = None;
        loop {
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            let result = self
                .transport
                .send_request(self.next_id(), "tools/list", Some(params))
                .await?;
            let page: ListToolsResult = serde_json::from_value(result)
                .map_err(|e| McpError::Protocol(format!("parse tools/list: {e}")))?;
            tools.extend(page.tools);
            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(Value::String(c)),
                _ => break,
            }
        }
        Ok(tools)
    }

    /// Invoke a tool by name with JSON arguments.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpCallResult, McpError> {
        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };
        let params_value =
            serde_json::to_value(&params).map_err(|e| McpError::Protocol(e.to_string()))?;
        let result = self
            .transport
            .send_request(self.next_id(), "tools/call", Some(params_value))
            .await?;
        let call: CallToolResult = serde_json::from_value(result)
            .map_err(|e| McpError::Protocol(format!("parse tools/call: {e}")))?;
        Ok(McpCallResult {
            content: call.content,
            is_error: call.is_error,
            structured_content: call.structured_content,
        })
    }
}

impl McpCallResult {
    /// Flatten content blocks into a single string suitable for the agent
    /// loop's `ToolResult` output. Text blocks are concatenated; binary blocks
    /// are summarised as placeholders.
    pub fn flatten_text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            let segment = match block {
                ContentBlock::Text { text } => text.clone(),
                ContentBlock::Image { data, mime_type } => {
                    format!(
                        "[image mime={} bytes={}]",
                        mime_type,
                        base64_decoded_len(data)
                    )
                }
                ContentBlock::Resource { resource } => {
                    format!("[resource uri={}]", resource.uri)
                }
            };
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&segment);
        }
        if out.is_empty() {
            if let Some(structured_content) = &self.structured_content {
                return serde_json::to_string(structured_content).unwrap_or_default();
            }
        }
        out
    }
}

fn base64_decoded_len(s: &str) -> usize {
    let len = s.len();
    if len == 0 || !len.is_multiple_of(4) {
        return len;
    }
    let padding = s.bytes().filter(|b| *b == b'=').count();
    (len / 4).saturating_mul(3).saturating_sub(padding)
}
