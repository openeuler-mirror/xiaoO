//! Input validation for FileWriteTool.
//!
//! Validates FileWriteInput before processing to ensure:
//! - UNC paths are allowed (like TypeScript)
//! - Secret patterns are detected

use std::path::Path;

use super::constants::{error_code, SECRET_DETECTED_MESSAGE, SECRET_PATTERNS};
use super::input::FileWriteInput;

/// Result of input validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether validation passed.
    pub result: bool,
    /// Error message if validation failed.
    pub message: Option<String>,
    /// Error code if validation failed.
    pub error_code: Option<u32>,
}

impl ValidationResult {
    /// Create a successful validation result.
    pub fn ok() -> Self {
        Self {
            result: true,
            message: None,
            error_code: None,
        }
    }

    /// Create a failed validation result.
    pub fn error(message: impl Into<String>, error_code: u32) -> Self {
        Self {
            result: false,
            message: Some(message.into()),
            error_code: Some(error_code),
        }
    }
}

/// Expands a file path:
/// - ~ is expanded to the home directory
/// - Relative paths are resolved to absolute paths
///
/// Uses std::env::var("HOME") for home directory (no external dependencies)
pub fn expand_path(path: &str) -> String {
    let path = path.trim();

    // Handle tilde expansion
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }

    // Handle relative paths
    if Path::new(path).is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join(path).to_string_lossy().into_owned();
        }
    }

    path.to_string()
}

/// Checks if a path is a UNC path (Windows network share path).
///
/// UNC paths start with `\\` (Windows-style) or `//` (Unix-style representation).
/// These paths refer to network resources and should be blocked for security reasons.
///
/// # Arguments
/// * `path` - The file path to check
///
/// # Returns
/// * `true` if the path is a UNC path, `false` otherwise
pub fn is_unc_path(path: &str) -> bool {
    let path = path.trim();
    path.starts_with("\\\\") || path.starts_with("//")
}

/// Checks if the content contains any secret patterns.
///
/// # Arguments
/// * `content` - The content to check
///
/// # Returns
/// * `true` if a secret pattern is detected, `false` otherwise
fn contains_secret(content: &str) -> bool {
    let content_lower = content.to_lowercase();
    for pattern in SECRET_PATTERNS {
        if content_lower.contains(pattern) {
            return true;
        }
    }
    false
}

/// Validates FileWriteInput for the FileWriteTool.
///
/// Checks:
/// - UNC paths are allowed (return ok)
/// - Basic secret pattern detection
///
/// # Arguments
/// * `input` - The FileWriteInput to validate
///
/// # Returns
/// * `ValidationResult` indicating whether the input is valid
pub fn validate_input(input: &FileWriteInput) -> ValidationResult {
    let expanded_path = expand_path(&input.file_path);

    // UNC paths are allowed (like TypeScript implementation)
    if is_unc_path(&expanded_path) {
        return ValidationResult::ok();
    }

    // Check for secret patterns in content
    if contains_secret(&input.content) {
        return ValidationResult::error(SECRET_DETECTED_MESSAGE, error_code::SECRET_DETECTED);
    }

    ValidationResult::ok()
}
