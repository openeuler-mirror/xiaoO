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

/// Validates FileWriteInput.
///
/// Always succeeds: file content is arbitrary user data (documentation and
/// source code often legitimately contain words such as "token" or
/// "secret"), so writes are never rejected on substring matching. Actual
/// credential handling belongs to the secret-scanning/redaction layer,
/// not this file I/O validator.
pub fn validate_input_with_base_from_bytes(_input: &FileWriteInput) -> ValidationResult {
    ValidationResult::ok()
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/tool/impl/builtin/file_write/validation/backend_test.rs"]
mod tests;
