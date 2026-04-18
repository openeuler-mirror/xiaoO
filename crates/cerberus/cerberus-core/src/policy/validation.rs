//! Policy validation types and logic.
//!
//! This module provides error types and validation utilities
//! for policy configuration.

use std::path::PathBuf;
use thiserror::Error;

/// Policy-related errors.
#[derive(Clone, Debug, Error)]
pub enum PolicyError {
    /// Failed to read policy file.
    #[error("Failed to read config file '{0}': {1}")]
    FileError(PathBuf, String),
    /// Failed to parse policy configuration.
    #[error("Failed to parse config: {0}")]
    ParseError(String),
    /// Failed to serialize policy configuration.
    #[error("Failed to serialize config: {0}")]
    SerializeError(String),
    /// Policy validation failed.
    #[error("Validation error: {0}")]
    ValidationError(String),
}
