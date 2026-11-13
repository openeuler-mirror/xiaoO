mod sse;
mod stdio;
mod streamable_http;

pub use sse::SseTransport;
pub use stdio::StdioTransport;
pub use streamable_http::StreamableHttpTransport;

use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::error::McpError;

/// Upper bound on the best-effort `notifications/cancelled` write. A server
/// that stops reading stdin must not make cancellation wait forever.
const CANCELLATION_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(1);

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

    /// Send a request and abandon it as soon as `cancel` fires.
    ///
    /// The default implementation races [`send_request`](Self::send_request)
    /// against the token and, on cancellation, sends the MCP
    /// `notifications/cancelled` for `id` before returning
    /// [`McpError::Cancelled`]. Transports that own a pending-request table
    /// override this so a dropped future can be cleaned up and a partially
    /// written frame can never be left behind (see the stdio transport).
    async fn send_request_cancellable(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        let Some(cancel) = cancel else {
            return self.send_request(id, method, params).await;
        };
        if cancel.is_cancelled() {
            return Err(McpError::Cancelled);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.send_cancelled_notification(id).await;
                Err(McpError::Cancelled)
            }
            result = self.send_request(id, method, params) => result,
        }
    }

    /// Best-effort `notifications/cancelled` for an in-flight request.
    ///
    /// MCP makes cancellation advisory: the receiver may already have
    /// completed the request, and a notification for an unknown id is
    /// ignored. A failed or slow write must therefore not turn cancellation
    /// into a different error — both the error and the join handle are
    /// discarded here.
    async fn send_cancelled_notification(&self, id: u64) {
        let params = serde_json::json!({ "requestId": id });
        let _ = tokio::time::timeout(
            CANCELLATION_NOTIFICATION_TIMEOUT,
            self.send_notification("notifications/cancelled", Some(params)),
        )
        .await;
    }

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
