//! Input validation for FileReadTool.
//!
//! Validates FileReadInput before processing to ensure:
//! - Pages parameter format is valid (e.g., "1-5", "3", "10-20")
//! - Pages range doesn't exceed maximum (100 pages per read)
//! - Binary files (non-PDF/image) are rejected
//! - Device paths are blocked

use super::constants::{
    IMAGE_PATH_SUFFIXES, PDF_PATH_SUFFIX, REJECTED_BINARY_EXTENSIONS, TEXT_FILE_EXTENSIONS,
};
use super::device::is_blocked_device_path;
use super::input::FileReadInput;

pub use super::constants::PDF_MAX_PAGES_PER_READ;

/// Error codes for validation failures.
pub mod error_code {
    /// Invalid pages format
    pub const INVALID_PAGES_FORMAT: u32 = 7;
    /// Pages range too large
    pub const PAGES_RANGE_TOO_LARGE: u32 = 8;
    /// Binary file rejected
    pub const BINARY_FILE_REJECTED: u32 = 4;
    /// Device path blocked
    pub const DEVICE_PATH_BLOCKED: u32 = 9;
}

/// Result of input validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub result: bool,
    pub message: Option<String>,
    pub error_code: Option<u32>,
}

impl ValidationResult {
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

/// Checks whether the path's extension marks it as a binary (non-text) file.
///
/// Extensions in `REJECTED_BINARY_EXTENSIONS` are always rejected; any other
/// 2–5 character extension that is neither a known text extension nor an
/// image/PDF suffix is treated as binary as well.
///
/// # Arguments
/// * `path` - The file path to check
///
/// # Returns
/// * `true` if the file is a binary file (should be rejected), `false` otherwise
fn is_binary_file_extension(path: &str) -> bool {
    let path_lower = path.to_lowercase();

    for ext in REJECTED_BINARY_EXTENSIONS {
        if path_lower.ends_with(ext) {
            return true;
        }
    }

    // Fall back to a heuristic for unknown extensions: image and PDF suffixes
    // are allowed, known text extensions are allowed, anything else looks
    // binary (e.g. .o, .obj, .class).
    if let Some(pos) = path_lower.rfind('.') {
        let ext = &path_lower[pos..];
        if ext == PDF_PATH_SUFFIX || IMAGE_PATH_SUFFIXES.contains(&ext) {
            return false;
        }
        if ext.len() > 1 && ext.len() <= 5 {
            if !TEXT_FILE_EXTENSIONS.contains(&ext) {
                return true;
            }
        }
    }

    false
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

    // Only the first comma-separated segment is parsed; the size over the
    // whole string is measured separately by `total_pages`.
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

/// Total number of pages a page-range string selects, counting **every**
/// comma-separated segment.
///
/// `parse_pages` only inspects the first segment (it returns a single
/// `(start, end)` pair), but `readers::pdf::parse_page_range` expands the whole
/// string, so the size that actually gets materialised has to be measured over
/// the whole string too. Duplicates are not subtracted: the reader dedups as it
/// expands, and an over-count here can only reject earlier, never admit more.
fn total_pages(pages: &str) -> Result<u64, &'static str> {
    let mut total: u64 = 0;
    for part in pages.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("Empty pages range");
        }
        let (start, end) = if part.contains('-') {
            let mut sides = part.split('-');
            let start: u32 = sides
                .next()
                .unwrap_or("")
                .trim()
                .parse()
                .map_err(|_| "Invalid start page")?;
            let end: u32 = sides
                .next()
                .unwrap_or("")
                .trim()
                .parse()
                .map_err(|_| "Invalid end page")?;
            if sides.next().is_some() {
                return Err("Invalid range format");
            }
            if start == 0 || end == 0 {
                return Err("Page numbers must be > 0");
            }
            if start > end {
                return Err("Start page must be <= end page");
            }
            (start, end)
        } else {
            let page: u32 = part.parse().map_err(|_| "Invalid page number")?;
            if page == 0 {
                return Err("Page numbers must be > 0");
            }
            (page, page)
        };
        total = total.saturating_add(u64::from(end - start) + 1);
    }
    Ok(total)
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
    if let Err(msg) = parse_pages(pages) {
        return ValidationResult::error(
            format!("Invalid pages format: {}", msg),
            error_code::INVALID_PAGES_FORMAT,
        );
    }
    match total_pages(pages) {
        Ok(range_size) if range_size > u64::from(PDF_MAX_PAGES_PER_READ) => {
            ValidationResult::error(
                format!(
                    "Pages range too large: {} pages requested, maximum is {}",
                    range_size, PDF_MAX_PAGES_PER_READ
                ),
                error_code::PAGES_RANGE_TOO_LARGE,
            )
        }
        Ok(_) => ValidationResult::ok(),
        Err(msg) => ValidationResult::error(
            format!("Invalid pages format: {}", msg),
            error_code::INVALID_PAGES_FORMAT,
        ),
    }
}

/// Validates FileReadInput using a pre-resolved path (backend path resolution).
///
/// This variant is used when path resolution has already been performed
/// by the backend, avoiding redundant host-local path expansion.
pub fn validate_input_with_base_from_bytes(
    input: &FileReadInput,
    resolved_path: &str,
) -> ValidationResult {
    if is_blocked_device_path(resolved_path) {
        return ValidationResult::error(
            format!("Device path is blocked: {}", resolved_path),
            error_code::DEVICE_PATH_BLOCKED,
        );
    }

    if is_binary_file_extension(resolved_path) {
        return ValidationResult::error(
            format!("Binary file rejected: {}", resolved_path),
            error_code::BINARY_FILE_REJECTED,
        );
    }

    if let Some(ref pages) = input.pages {
        let pages_result = validate_pages(pages);
        if !pages_result.result {
            return pages_result;
        }
    }

    ValidationResult::ok()
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/tool/impl/builtin/file_read/validation/backend_test.rs"]
mod tests;
