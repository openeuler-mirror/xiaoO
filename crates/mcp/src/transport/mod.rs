mod sse;
mod stdio;
mod streamable_http;

pub use sse::SseTransport;
pub use stdio::StdioTransport;
pub use streamable_http::StreamableHttpTransport;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::McpError;

/// Bidirectional JSON-RPC transport. Implementations own the underlying
/// connection (child process pipes, HTTP stream) and serialise requests.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and wait for the matching response.
    async fn send_request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError>;

    /// Send a fire-and-forget notification.
    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError>;

    /// Protocol version advertised in the initialize request.
    fn initialize_protocol_version(&self) -> &'static str {
        "2024-11-05"
    }

    /// Validate the version negotiated by the server.
    fn validate_negotiated_protocol_version(&self, _version: &str) -> Result<(), McpError> {
        Ok(())
    }

    /// Record the protocol version selected during initialisation. Transports
    /// that do not negotiate protocol headers deliberately ignore this.
    async fn set_protocol_version(&self, _protocol_version: &str) {}

    /// Release transport-owned resources. Legacy transports do not require an
    /// explicit close, while Streamable HTTP uses this to terminate its MCP
    /// session before the process exits.
    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/mcp/transport_test.rs"]
mod tests;
