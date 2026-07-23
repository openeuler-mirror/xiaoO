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
}

#[cfg(test)]
mod tests {
    use super::McpTransport;
    use crate::McpError;
    use async_trait::async_trait;
    use serde_json::Value;

    struct LegacyTransportDefaults;

    #[async_trait]
    impl McpTransport for LegacyTransportDefaults {
        async fn send_request(
            &self,
            _id: u64,
            _method: &str,
            _params: Option<Value>,
        ) -> Result<Value, McpError> {
            unreachable!()
        }

        async fn send_notification(
            &self,
            _method: &str,
            _params: Option<Value>,
        ) -> Result<(), McpError> {
            unreachable!()
        }
    }

    #[test]
    fn legacy_transports_keep_the_original_protocol_version_contract() {
        let transport = LegacyTransportDefaults;

        assert_eq!(transport.initialize_protocol_version(), "2024-11-05");
        assert!(transport
            .validate_negotiated_protocol_version("2024-11-05")
            .is_ok());
        assert!(transport
            .validate_negotiated_protocol_version("2025-11-25")
            .is_ok());
    }
}
