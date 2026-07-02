//! Input validation for FileWriteTool.
//!
//! Validates FileWriteInput before processing to ensure:
//! - Secret patterns are detected

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

/// Validates FileWriteInput using a pre-resolved path (backend path resolution).
///
/// This variant is used when path resolution has already been performed
/// by the backend, avoiding redundant host-local path expansion.
pub fn validate_input_with_base_from_bytes(input: &FileWriteInput) -> ValidationResult {
    if contains_secret(&input.content) {
        return ValidationResult::error(SECRET_DETECTED_MESSAGE, error_code::SECRET_DETECTED);
    }

    ValidationResult::ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_secret_content_for_unc_like_path() {
        let input = FileWriteInput {
            file_path: "//tmp/config.txt".to_string(),
            content: "api_key = \"sk-demo\"".to_string(),
        };

        let result = validate_input_with_base_from_bytes(&input);

        assert!(!result.result);
        assert_eq!(result.error_code, Some(error_code::SECRET_DETECTED));
    }
}
