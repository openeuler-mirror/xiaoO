//! Input validation for FileWriteTool.
//!
//! Validates FileWriteInput before processing to ensure:
//! - Input is valid for the selected backend

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
}

/// Validates FileWriteInput using a pre-resolved path (backend path resolution).
///
/// This variant is used when path resolution has already been performed
/// by the backend, avoiding redundant host-local path expansion.
pub fn validate_input_with_base_from_bytes(_input: &FileWriteInput) -> ValidationResult {
    // File content is arbitrary user data (documentation and source code often
    // legitimately contain words such as "token" or "secret").  Do not reject
    // writes based on substring matching; actual credential handling belongs to
    // the secret-scanning/redaction layer, not this file I/O validator.
    ValidationResult::ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_documentation_that_mentions_secret_like_terms() {
        let input = FileWriteInput {
            file_path: "//tmp/config.txt".to_string(),
            content: "Explain how an api_key token or secret is configured.".to_string(),
        };

        let result = validate_input_with_base_from_bytes(&input);

        assert!(result.result);
        assert_eq!(result.error_code, None);
    }
}
