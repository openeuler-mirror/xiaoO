//! Input validation for FileEditTool.
//!
//! Validates FileEditInput before processing to ensure:
//! - No change check (old_string == new_string)
//! - File exists when editing
//! - File is not a notebook file
//! - File size is within limits
//! - File was not modified since read (mtime tracking)
//! - old_string exists in file
//! - Ambiguous match check (multiple matches but replace_all=false)
//! - Secret patterns not present in edit strings

use std::path::Path;

use super::constants::MAX_EDIT_FILE_SIZE;
use super::input::FileEditInput;
use super::utils::find_actual_string;
use crate::r#impl::builtin::file_read::dedup::DedupStateStore;
use agent_contracts::backend::PathStat;

/// Error codes for validation failures.
///
/// These codes MUST match the TypeScript error codes exactly.
pub mod error_code {
    /// Secret pattern detected (error_code = 0)
    pub const SECRET_DETECTED: u32 = 0;
    /// No change made (old_string == new_string) (error_code = 1)
    pub const NO_CHANGE: u32 = 1;
    /// File exists when trying to create with empty old_string (error_code = 3)
    pub const FILE_EXISTS: u32 = 3;
    /// File not found (error_code = 4)
    pub const FILE_NOT_FOUND: u32 = 4;
    /// Notebook file not supported (error_code = 5)
    pub const NOTEBOOK_FILE: u32 = 5;
    /// File modified since read (error_code = 7)
    pub const FILE_MODIFIED: u32 = 7;
    /// old_string not found in file (error_code = 8)
    pub const OLD_STRING_NOT_FOUND: u32 = 8;
    /// Ambiguous match (multiple matches but replace_all=false) (error_code = 9)
    pub const AMBIGUOUS_MATCH: u32 = 9;
    /// File too large (error_code = 10)
    pub const FILE_TOO_LARGE: u32 = 10;
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

/// Notebook file extension.
const NOTEBOOK_EXTENSION: &str = "ipynb";

/// Secret patterns that should not be edited.
///
/// These patterns indicate potentially sensitive data that should not be modified.
const SECRET_PATTERNS: &[&str] = &[
    "API_KEY",
    "api_key",
    "API-KEY",
    "SECRET",
    "PASSWORD",
    "PWD",
    "TOKEN",
    "ACCESS_TOKEN",
    "AUTH_TOKEN",
    "PRIVATE_KEY",
    "AWS_ACCESS_KEY",
    "AWS_SECRET_KEY",
    "STRIPE_KEY",
    "GITHUB_TOKEN",
];

/// Checks if a string contains a secret pattern.
///
/// # Arguments
/// * `s` - The string to check
///
/// # Returns
/// * `true` if a secret pattern is found, `false` otherwise
fn contains_secret(s: &str) -> bool {
    let upper = s.to_uppercase();
    for pattern in SECRET_PATTERNS {
        if upper.contains(&pattern.to_uppercase()) {
            return true;
        }
    }
    false
}

/// Checks if a file extension indicates a notebook file.
///
/// # Arguments
/// * `path` - The file path to check
///
/// # Returns
/// * `true` if the file is a notebook file, `false` otherwise
fn is_notebook_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(NOTEBOOK_EXTENSION))
        .unwrap_or(false)
}

/// Counts occurrences of a string in content.
///
/// # Arguments
/// * `content` - The content to search
/// * `search_string` - The string to count
///
/// # Returns
/// * Number of occurrences
fn count_occurrences(content: &str, search_string: &str) -> usize {
    content.match_indices(search_string).count()
}

/// Validates that old_string doesn't match new_string.
///
/// # Arguments
/// * `input` - The FileEditInput to validate
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_no_change(input: &FileEditInput) -> ValidationResult {
    if input.old_string == input.new_string {
        return ValidationResult::error(
            "No change: old_string and new_string are identical",
            error_code::NO_CHANGE,
        );
    }
    ValidationResult::ok()
}

/// Validates that the file doesn't contain secret patterns.
///
/// # Arguments
/// * `input` - The FileEditInput to validate
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_no_secrets(input: &FileEditInput) -> ValidationResult {
    if contains_secret(&input.old_string) {
        return ValidationResult::error(
            "Secret pattern detected in old_string",
            error_code::SECRET_DETECTED,
        );
    }
    if contains_secret(&input.new_string) {
        return ValidationResult::error(
            "Secret pattern detected in new_string",
            error_code::SECRET_DETECTED,
        );
    }
    ValidationResult::ok()
}

/// Validates that the file is not a notebook file.
///
/// # Arguments
/// * `expanded_path` - The expanded file path
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_not_notebook(expanded_path: &str) -> ValidationResult {
    if is_notebook_file(expanded_path) {
        return ValidationResult::error(
            format!(
                "Notebook files (.ipynb) cannot be edited: {}",
                expanded_path
            ),
            error_code::NOTEBOOK_FILE,
        );
    }
    ValidationResult::ok()
}

/// Validates FileEditInput using backend-provided stat and mtime.
///
/// This variant is used when path resolution and file stat have already been
/// performed by the backend, avoiding redundant host-local filesystem operations.
pub fn validate_input_backend(
    input: &FileEditInput,
    content: Option<&str>,
    dedup_store: &DedupStateStore,
    resolved_path: &str,
    stat: &PathStat,
    mtime: i64,
) -> ValidationResult {
    let result = validate_no_change(input);
    if !result.result {
        return result;
    }

    let result = validate_no_secrets(input);
    if !result.result {
        return result;
    }

    let result = validate_not_notebook(resolved_path);
    if !result.result {
        return result;
    }

    if !input.old_string.is_empty() {
        if !stat.exists {
            return ValidationResult::error(
                format!("File not found: {}", resolved_path),
                error_code::FILE_NOT_FOUND,
            );
        }

        if let Some(size) = stat.size_bytes {
            if size > MAX_EDIT_FILE_SIZE {
                return ValidationResult::error(
                    format!(
                        "File too large: {} bytes (max: {} bytes)",
                        size, MAX_EDIT_FILE_SIZE
                    ),
                    error_code::FILE_TOO_LARGE,
                );
            }
        }
    } else {
        if stat.exists {
            let file_empty = match content {
                Some(c) => c.is_empty(),
                None => true,
            };

            if !file_empty {
                return ValidationResult::error(
                    format!(
                        "Cannot create file with content when file exists and is not empty: {}",
                        resolved_path
                    ),
                    error_code::FILE_EXISTS,
                );
            }
        }
    }

    if !input.old_string.is_empty() {
        if let Some(state) = dedup_store.get_read_state(resolved_path) {
            if state.timestamp != mtime {
                return ValidationResult::error(
                    format!(
                        "File modified since read: {} (mtime changed from {} to {})",
                        resolved_path, state.timestamp, mtime
                    ),
                    error_code::FILE_MODIFIED,
                );
            }
        }
    }

    if let Some(c) = content {
        if !input.old_string.is_empty() {
            let result = validate_old_string_exists(c, input);
            if !result.result {
                return result;
            }

            let result = validate_ambiguous_match(c, input);
            if !result.result {
                return result;
            }
        }
    }

    ValidationResult::ok()
}

/// Validates that old_string exists in the file content.
///
/// Uses find_actual_string to handle quote normalization.
///
/// # Arguments
/// * `content` - The file content
/// * `input` - The FileEditInput with old_string
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_old_string_exists(content: &str, input: &FileEditInput) -> ValidationResult {
    let actual_string = find_actual_string(content, &input.old_string);
    if actual_string.is_none() {
        return ValidationResult::error(
            format!("old_string not found in file: {}", input.old_string),
            error_code::OLD_STRING_NOT_FOUND,
        );
    }
    ValidationResult::ok()
}

/// Validates that there's no ambiguous match.
///
/// If replace_all is false and there are multiple occurrences,
/// this is considered ambiguous.
///
/// # Arguments
/// * `content` - The file content
/// * `input` - The FileEditInput with old_string and replace_all flag
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_ambiguous_match(content: &str, input: &FileEditInput) -> ValidationResult {
    if input.replace_all {
        return ValidationResult::ok();
    }

    let actual_string = match find_actual_string(content, &input.old_string) {
        Some(s) => s,
        None => return ValidationResult::ok(), // old_string not found - other check will catch it
    };

    let occurrences = count_occurrences(content, &actual_string);
    if occurrences > 1 {
        return ValidationResult::error(
            format!(
                "Ambiguous match: old_string appears {} times but replace_all is false",
                occurrences
            ),
            error_code::AMBIGUOUS_MATCH,
        );
    }
    ValidationResult::ok()
}
