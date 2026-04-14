//! Device file path blocking for file read operations.
//!
//! This module provides path validation to prevent reading from device files
//! that could produce infinite output or other problematic behavior.

use super::constants::BLOCKED_DEVICE_PATHS;

/// Checks if a path refers to a blocked device file.
///
/// # Arguments
/// * `path` - The file path to check
///
/// # Returns
/// * `true` if the path is a blocked device, `false` otherwise
///
/// # Examples
/// ```
/// use tool::r#impl::file_read::device::is_blocked_device_path;
///
/// assert!(is_blocked_device_path("/dev/zero"));
/// assert!(is_blocked_device_path("/dev/urandom"));
/// assert!(is_blocked_device_path("/proc/self/fd/0"));
/// assert!(!is_blocked_device_path("/dev/null")); // /dev/null is safe
/// assert!(!is_blocked_device_path("/etc/passwd"));
/// ```
pub fn is_blocked_device_path(path: &str) -> bool {
    // Fast path: check exact matches first
    if BLOCKED_DEVICE_PATHS.iter().any(|&p| path == p) {
        return true;
    }

    // Check /proc/self/fd/* patterns
    if path.starts_with("/proc/self/fd/") {
        if let Some(fd) = path.strip_prefix("/proc/self/fd/") {
            // Block fd/0, fd/1, fd/2
            if fd == "0" || fd == "1" || fd == "2" {
                return true;
            }
        }
    }

    // Check /proc/<pid>/fd/* patterns
    // Matches /proc/12345/fd/0, /proc/12345/fd/1, /proc/12345/fd/2
    if path.starts_with("/proc/") && path.contains("/fd/") {
        if let Some(fd_part) = path.strip_prefix("/proc/") {
            if let Some(rest) = fd_part.find("/fd/") {
                let pid_and_rest = &fd_part[..rest];
                let fd = &fd_part[rest + 4..];
                // Verify the PID part is digits only (basic validation)
                if pid_and_rest.chars().all(|c| c.is_ascii_digit()) {
                    if fd == "0" || fd == "1" || fd == "2" {
                        return true;
                    }
                }
            }
        }
    }

    false
}
