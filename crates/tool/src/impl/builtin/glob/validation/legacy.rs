//! Input validation for GlobTool.
//!
//! Validates GlobInput before processing to ensure:
//! - Path exists (if provided)
//! - Path is a directory (if provided)
//! - UNC paths are passed through without validation

use std::path::Path;

use crate::r#impl::path_resolver::expand_path_from_base;

use super::input::GlobInput;

/// Error codes for validation failures.
pub mod error_code {
    /// Directory not found (error_code = 1)
    pub const DIRECTORY_NOT_FOUND: u32 = 1;
    /// Path is not a directory (error_code = 2)
    pub const NOT_A_DIRECTORY: u32 = 2;
}

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
/// - UNC paths (// or \\) are preserved as-is for security
///
/// Uses std::env::current_dir() for path resolution (no external dependencies)
pub fn expand_path(path: &str, base_dir: &Path) -> String {
    let path = path.trim();

    // SECURITY: Preserve UNC paths as-is (network shares like \\server\share)
    // These should not be modified or joined with CWD
    if path.starts_with("\\\\") || path.starts_with("//") {
        return path.to_string();
    }

    expand_path_from_base(path, base_dir)
}

/// Checks if a path is a UNC path (network share path).
///
/// UNC paths start with \\ or // and should be passed through without validation.
fn is_unc_path(path: &str) -> bool {
    path.starts_with("\\\\") || path.starts_with("//")
}

/// Validates GlobInput for the GlobTool.
///
/// Checks:
/// - Path exists (if provided)
/// - Path is a directory (if provided)
/// - UNC paths are passed through without validation
///
/// # Arguments
/// * `input` - The GlobInput to validate
///
/// # Returns
/// * `ValidationResult` indicating whether the input is valid
pub fn validate_input_with_base(input: &GlobInput, base_dir: &Path) -> ValidationResult {
    if let Some(ref path) = input.path {
        let absolute_path = expand_path(path, base_dir);

        // SECURITY: Skip validation for UNC paths (network shares)
        if is_unc_path(&absolute_path) {
            return ValidationResult::ok();
        }

        // Check if path exists
        if !Path::new(&absolute_path).exists() {
            return ValidationResult::error(
                format!("Directory does not exist: {}", path),
                error_code::DIRECTORY_NOT_FOUND,
            );
        }

        // Check if path is a directory
        if !Path::new(&absolute_path).is_dir() {
            return ValidationResult::error(
                format!("Path is not a directory: {}", path),
                error_code::NOT_A_DIRECTORY,
            );
        }
    }

    ValidationResult::ok()
}
