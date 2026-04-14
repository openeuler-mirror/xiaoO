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
mod tests {
    use super::*;

    #[test]
    fn test_from_rusqlite_error() {
        let db_error = rusqlite::Error::ExecuteReturnedResults;
        let converted: MoiraiError = db_error.into();

        // Verify it's the Storage variant
        match converted {
            MoiraiError::Storage(msg) => {
                assert!(!msg.is_empty());
            }
            _ => panic!("Expected Storage variant"),
        }
    }

    #[test]
    fn test_from_serde_error() {
        let json_str = "{invalid json}";
        let serde_error: serde_json::Error =
            serde_json::from_str::<serde_json::Value>(json_str).unwrap_err();
        let converted: MoiraiError = serde_error.into();

        // Verify it's the Serialization variant
        match converted {
            MoiraiError::Serialization(msg) => {
                assert!(!msg.is_empty());
            }
            _ => panic!("Expected Serialization variant"),
        }
    }

    #[test]
    fn test_error_display() {
        // Test Storage variant
        let storage_err = MoiraiError::Storage("db connection failed".to_string());
        assert_eq!(
            storage_err.to_string(),
            "Storage error: db connection failed"
        );

        // Test Serialization variant
        let serde_err = MoiraiError::Serialization("invalid json".to_string());
        assert_eq!(serde_err.to_string(), "Serialization error: invalid json");

        // Test InvalidState variant
        let state_err = MoiraiError::InvalidState("already initialized".to_string());
        assert_eq!(state_err.to_string(), "Invalid state: already initialized");

        // Test NotFound variant
        let not_found_err = MoiraiError::NotFound("trace not found".to_string());
        assert_eq!(not_found_err.to_string(), "Not found: trace not found");
    }
}
