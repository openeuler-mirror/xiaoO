//! Input validation for FileReadTool.
//!
//! Validates FileReadInput before processing to ensure:
//! - Pages parameter format is valid (e.g., "1-5", "3", "10-20")
//! - Pages range doesn't exceed maximum (100 pages per read)
//! - Binary files (non-PDF/image) are rejected
//! - Device paths are blocked
use std::path::Path;

use crate::r#impl::path_resolver::expand_path_from_base;

use super::constants::{
    IMAGE_PATH_SUFFIXES, PDF_PATH_SUFFIX, REJECTED_BINARY_EXTENSIONS, TEXT_FILE_EXTENSIONS,
};
use super::device::is_blocked_device_path;
use super::input::FileReadInput;

pub use super::constants::PDF_MAX_PAGES_PER_READ;

/// Error codes for validation failures.
pub mod error_code {
    /// Invalid pages format (error_code = 7)
    pub const INVALID_PAGES_FORMAT: u32 = 7;
    /// Pages range too large (error_code = 8)
    pub const PAGES_RANGE_TOO_LARGE: u32 = 8;
    /// Binary file rejected (error_code = 4)
    pub const BINARY_FILE_REJECTED: u32 = 4;
    /// Device path blocked (error_code = 9)
    pub const DEVICE_PATH_BLOCKED: u32 = 9;
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

/// Binary file extensions that are explicitly rejected.
///
/// These extensions represent non-text, non-PDF, non-image binary files.
/// Checks if a file extension indicates a binary file.
///
/// Binary files are those with extensions in BINARY_EXTENSIONS,
/// excluding image and PDF extensions.
///
/// # Arguments
/// * `path` - The file path to check
///
/// # Returns
/// * `true` if the file is a binary file (should be rejected), `false` otherwise
fn is_binary_file_extension(path: &str) -> bool {
    let path_lower = path.to_lowercase();

    // Check if it's a known binary extension
    for ext in REJECTED_BINARY_EXTENSIONS {
        if path_lower.ends_with(ext) {
            return true;
        }
    }

    // If it has an extension but is not an image or PDF, treat as binary
    // This catches extensions like .o, .obj, .class, etc.
    if let Some(pos) = path_lower.rfind('.') {
        let ext = &path_lower[pos..];
        // Allow image and PDF extensions
        if ext == PDF_PATH_SUFFIX || IMAGE_PATH_SUFFIXES.contains(&ext) {
            return false;
        }
        // Any other extension with length > 1 is likely a binary
        if ext.len() > 1 && ext.len() <= 5 {
            // Check if it's a text-like extension
            if !TEXT_FILE_EXTENSIONS.contains(&ext) {
                return true;
            }
        }
    }

    false
}

/// Expands a file path:
/// - ~ is expanded to the home directory
/// - Relative paths are resolved to absolute paths
///
/// Uses std::env::var("HOME") for home directory (no external dependencies)

fn expand_path(path: &str, base_dir: &Path) -> String {
    expand_path_from_base(path, base_dir)
}

/// Validates the pages parameter format.
///
/// Accepts formats like:
/// - Single page: "3"
/// - Range: "1-5"
/// - Multiple ranges/singles: "1-5, 3, 10-20"
///
/// # Arguments
/// * `pages` - The pages string to validate
///
/// # Returns
/// * `Ok((start_page, end_page))` for single ranges
/// * `Err(&str)` for invalid format
fn parse_pages(pages: &str) -> Result<(u32, u32), &'static str> {
    let pages = pages.trim();

    // Handle multiple ranges (take the first range for size validation)
    let first_range = if pages.contains(',') {
        pages.split(',').next().unwrap_or("")
    } else {
        pages
    };

    let range = first_range.trim();

    if range.is_empty() {
        return Err("Empty pages range");
    }

    if range.contains('-') {
        let parts: Vec<&str> = range.split('-').collect();
        if parts.len() != 2 {
            return Err("Invalid range format");
        }
        let start: u32 = parts[0].trim().parse().map_err(|_| "Invalid start page")?;
        let end: u32 = parts[1].trim().parse().map_err(|_| "Invalid end page")?;

        if start == 0 || end == 0 {
            return Err("Page numbers must be > 0");
        }
        if start > end {
            return Err("Start page must be <= end page");
        }

        Ok((start, end))
    } else {
        // Single page
        let page: u32 = range.parse().map_err(|_| "Invalid page number")?;
        if page == 0 {
            return Err("Page numbers must be > 0");
        }
        Ok((page, page))
    }
}

/// Validates the pages parameter.
///
/// Checks:
/// - Format is valid (e.g., "1-5", "3", "10-20")
/// - Range size doesn't exceed PDF_MAX_PAGES_PER_READ
///
/// # Arguments
/// * `pages` - The pages string to validate
///
/// # Returns
/// * `ValidationResult` indicating success or failure with error details
fn validate_pages(pages: &str) -> ValidationResult {
    match parse_pages(pages) {
        Ok((start, end)) => {
            let range_size = end - start + 1;
            if range_size > PDF_MAX_PAGES_PER_READ {
                ValidationResult::error(
                    format!(
                        "Pages range too large: {} pages requested, maximum is {}",
                        range_size, PDF_MAX_PAGES_PER_READ
                    ),
                    error_code::PAGES_RANGE_TOO_LARGE,
                )
            } else {
                ValidationResult::ok()
            }
        }
        Err(msg) => ValidationResult::error(
            format!("Invalid pages format: {}", msg),
            error_code::INVALID_PAGES_FORMAT,
        ),
    }
}

/// Validates FileReadInput for the FileReadTool.
///
/// Checks:
/// - Pages parameter format (if provided)
/// - Binary file extension (rejects non-PDF/image binary)
/// - Device path blocking
///
/// # Arguments
/// * `input` - The FileReadInput to validate
///
/// # Returns
/// * `ValidationResult` indicating whether the input is valid
pub fn validate_input_with_base(input: &FileReadInput, base_dir: &Path) -> ValidationResult {
    let expanded_path = expand_path(&input.file_path, base_dir);

    // Check device path first
    if is_blocked_device_path(&expanded_path) {
        return ValidationResult::error(
            format!("Device path is blocked: {}", expanded_path),
            error_code::DEVICE_PATH_BLOCKED,
        );
    }

    // Check binary file extension
    if is_binary_file_extension(&expanded_path) {
        return ValidationResult::error(
            format!("Binary file rejected: {}", expanded_path),
            error_code::BINARY_FILE_REJECTED,
        );
    }

    // Validate pages parameter if provided
    if let Some(ref pages) = input.pages {
        let pages_result = validate_pages(pages);
        if !pages_result.result {
            return pages_result;
        }
    }

    ValidationResult::ok()
}
