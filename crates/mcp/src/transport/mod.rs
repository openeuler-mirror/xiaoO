mod sse;
mod stdio;

pub use sse::SseTransport;
pub use stdio::StdioTransport;

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
}
