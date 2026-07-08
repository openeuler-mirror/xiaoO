use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("failed to spawn mcp server '{command}': {error}")]
    SpawnFailed { command: String, error: String },

    #[error("mcp handshake failed: {0}")]
    HandshakeFailed(String),

    #[error("mcp transport closed")]
    TransportClosed,

    #[error("mcp server returned error {code}: {message}")]
    ServerError { code: i64, message: String },

    #[error("mcp request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("mcp protocol error: {0}")]
    Protocol(String),

    #[error("mcp io error: {0}")]
    Io(String),

    #[error("mcp http error: {0}")]
    Http(String),

    #[error("mcp server disconnected during request")]
    Disconnected,
}

impl From<std::io::Error> for McpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
