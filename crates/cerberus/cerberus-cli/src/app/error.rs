//! CLI error types.

use cerberus_core::{CerberusError, FilterError, SandboxSetupError};
use thiserror::Error;

/// CLI error type.
#[derive(Debug, Error)]
pub enum CliError {
    /// Storage error.
    #[error("Storage error: {0}")]
    StorageError(String),

    /// IO error.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// SQLite error.
    #[error("Database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Execution error.
    #[error("Execution error: {0}")]
    ExecutionError(#[from] cerberus_core::CerberusError),

    /// Host adapter error.
    #[error("Host adapter error: {0}")]
    AdapterError(String),

    /// JSON parsing error.
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Returns the human-readable reason when execution was blocked before completion.
pub fn blocked_reason(error: &CerberusError) -> Option<&str> {
    match error {
        CerberusError::Filter(FilterError::RejectedArgument { reason, .. })
        | CerberusError::Filter(FilterError::RejectedEnvironment { reason, .. })
        | CerberusError::Filter(FilterError::ViolationTriggered { reason, .. }) => Some(reason),
        CerberusError::SandboxSetup(SandboxSetupError::CapabilityError { reason, .. }) => {
            Some(reason)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::blocked_reason;
    use cerberus_core::{CerberusError, FilterError, SandboxSetupError};

    #[test]
    fn blocked_reason_extracts_filter_reason() {
        let error = CerberusError::Filter(FilterError::RejectedEnvironment {
            name: "SECRET".into(),
            reason: "Environment variable 'SECRET' is in deny list".into(),
        });

        assert_eq!(
            blocked_reason(&error),
            Some("Environment variable 'SECRET' is in deny list")
        );
    }

    #[test]
    fn blocked_reason_extracts_capability_reason() {
        let error = CerberusError::SandboxSetup(SandboxSetupError::CapabilityError {
            feature: "namespaces".into(),
            reason: "missing user namespace support".into(),
        });

        assert_eq!(
            blocked_reason(&error),
            Some("missing user namespace support")
        );
    }
}
