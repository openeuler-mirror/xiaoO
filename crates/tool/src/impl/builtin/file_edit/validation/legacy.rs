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

use crate::r#impl::path_resolver::expand_path_from_base;

use super::constants::MAX_EDIT_FILE_SIZE;
use super::input::FileEditInput;
use super::utils::find_actual_string;
use crate::r#impl::builtin::file_read::dedup::{get_file_mtime, DedupStateStore};

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

/// Expands a file path:
/// - ~ is expanded to the home directory
/// - Relative paths are resolved to absolute paths
///
/// Uses std::env::var("HOME") for home directory (no external dependencies)
pub fn expand_path(path: &str, base_dir: &Path) -> String {
    expand_path_from_base(path, base_dir)
}

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

/// Validates that the file exists.
///
/// # Arguments
/// * `expanded_path` - The expanded file path
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_file_exists(expanded_path: &str) -> ValidationResult {
    if !Path::new(expanded_path).exists() {
        return ValidationResult::error(
            format!("File not found: {}", expanded_path),
            error_code::FILE_NOT_FOUND,
        );
    }
    ValidationResult::ok()
}

/// Validates file size is within limits.
///
/// # Arguments
/// * `expanded_path` - The expanded file path
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_file_size(expanded_path: &str) -> ValidationResult {
    match std::fs::metadata(expanded_path) {
        Ok(metadata) => {
            if metadata.len() > MAX_EDIT_FILE_SIZE {
                return ValidationResult::error(
                    format!(
                        "File too large: {} bytes (max: {} bytes)",
                        metadata.len(),
                        MAX_EDIT_FILE_SIZE
                    ),
                    error_code::FILE_TOO_LARGE,
                );
            }
            ValidationResult::ok()
        }
        Err(_) => ValidationResult::ok(), // File existence check will catch this
    }
}

/// Validates that the file was not modified since read.
///
/// Uses mtime comparison to detect changes.
///
/// # Arguments
/// * `expanded_path` - The expanded file path
/// * `dedup_store` - Reference to the dedup state store
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_file_not_modified(
    expanded_path: &str,
    dedup_store: &DedupStateStore,
) -> ValidationResult {
    let Some(state) = dedup_store.get_read_state(expanded_path) else {
        // Skip mtime validation when there is no prior read state to compare against.
        return ValidationResult::ok();
    };

    let current_mtime = match get_file_mtime(Path::new(expanded_path)) {
        Some(mtime) => mtime,
        None => return ValidationResult::ok(), // File might not exist, other checks will catch it
    };

    if state.timestamp != current_mtime {
        return ValidationResult::error(
            format!(
                "File modified since read: {} (mtime changed from {} to {})",
                expanded_path, state.timestamp, current_mtime
            ),
            error_code::FILE_MODIFIED,
        );
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

/// Validates that when old_string is empty, the file is empty or doesn't exist.
///
/// This is for creating a new file - if old_string is empty but new_string is not,
/// and the file exists with content, that's an error.
///
/// # Arguments
/// * `expanded_path` - The expanded file path
/// * `content` - The file content (if file was read)
/// * `input` - The FileEditInput
///
/// # Returns
/// * `ValidationResult` indicating success or failure
fn validate_create_new_file(
    expanded_path: &str,
    content: Option<&str>,
    input: &FileEditInput,
) -> ValidationResult {
    // Only applies when old_string is empty and new_string is not
    if !input.old_string.is_empty() || input.new_string.is_empty() {
        return ValidationResult::ok();
    }

    // If file doesn't exist, this is a create operation - allowed
    if !Path::new(expanded_path).exists() {
        return ValidationResult::ok();
    }

    // File exists - check if it's empty
    let file_empty = match content {
        Some(c) => c.is_empty(),
        None => {
            // Read the file to check if it's empty
            match std::fs::read_to_string(expanded_path) {
                Ok(content) => content.is_empty(),
                Err(_) => return ValidationResult::ok(), // Can't read, let executor handle it
            }
        }
    };

    if !file_empty {
        return ValidationResult::error(
            format!(
                "Cannot create file with content when file exists and is not empty: {}",
                expanded_path
            ),
            error_code::FILE_EXISTS,
        );
    }

    ValidationResult::ok()
}

/// Validates FileEditInput for the FileEditTool.
///
/// This is the main validation function that checks:
/// - No change (old_string == new_string)
/// - Secret patterns not present
/// - File is not a notebook
/// - File exists (for edit operations)
/// - File size is within limits
///
/// For checks that require file content (old_string exists, ambiguous match)
/// and state tracking (file modified after a prior read), additional context is needed.
///
/// # Arguments
/// * `input` - The FileEditInput to validate
/// * `content` - Optional file content for string matching checks
/// * `dedup_store` - Optional reference to dedup state store for read tracking
///
/// # Returns
/// * `ValidationResult` indicating whether the input is valid
pub fn validate_input(
    input: &FileEditInput,
    content: Option<&str>,
    dedup_store: Option<&DedupStateStore>,
    base_dir: &Path,
) -> ValidationResult {
    let expanded_path = expand_path(&input.file_path, base_dir);

    // Check: No change (old_string == new_string)
    let result = validate_no_change(input);
    if !result.result {
        return result;
    }

    // Check: Secret patterns
    let result = validate_no_secrets(input);
    if !result.result {
        return result;
    }

    // Check: File is not a notebook
    let result = validate_not_notebook(&expanded_path);
    if !result.result {
        return result;
    }

    // For edit operations (old_string is not empty), file must exist
    if !input.old_string.is_empty() {
        let result = validate_file_exists(&expanded_path);
        if !result.result {
            return result;
        }

        // Check: File size
        let result = validate_file_size(&expanded_path);
        if !result.result {
            return result;
        }
    } else {
        // For create operations (old_string is empty), check file doesn't exist or is empty
        let result = validate_create_new_file(&expanded_path, content, input);
        if !result.result {
            return result;
        }
    }

    // Check: File was not modified since a prior read (requires dedup_store)
    if let Some(store) = dedup_store {
        // Only check if old_string is not empty (edit operation)
        if !input.old_string.is_empty() {
            let result = validate_file_not_modified(&expanded_path, store);
            if !result.result {
                return result;
            }
        }
    }

    // Check: old_string exists in file (requires content)
    if let Some(c) = content {
        // Only check if old_string is not empty
        if !input.old_string.is_empty() {
            let result = validate_old_string_exists(c, input);
            if !result.result {
                return result;
            }

            // Check: Ambiguous match
            let result = validate_ambiguous_match(c, input);
            if !result.result {
                return result;
            }
        }
    }

    ValidationResult::ok()
}
