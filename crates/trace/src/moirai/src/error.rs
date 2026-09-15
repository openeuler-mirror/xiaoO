use thiserror::Error;

/// Moirai tracing SDK error types
#[derive(Error, Debug)]
pub enum MoiraiError {
    /// Database or storage operation failed
    #[error("Storage error: {0}")]
    Storage(String),

    /// JSON serialization/deserialization failed
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Invalid operation state
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Resource not found
    #[error("Not found: {0}")]
    NotFound(String),
}

impl From<rusqlite::Error> for MoiraiError {
    fn from(err: rusqlite::Error) -> Self {
        MoiraiError::Storage(err.to_string())
    }
}

impl From<serde_json::Error> for MoiraiError {
    fn from(err: serde_json::Error) -> Self {
        MoiraiError::Serialization(err.to_string())
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/trace/moirai/error_test.rs"]
mod tests;
