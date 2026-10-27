//! Device file path blocking for file read operations.
//!
//! This module provides path validation to prevent reading from device files
//! that could produce infinite output or other problematic behavior.

use super::constants::BLOCKED_DEVICE_PATHS;

/// Checks if a path refers to a blocked device file.
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
    if BLOCKED_DEVICE_PATHS.contains(&path) {
        return true;
    }

    if path.starts_with("/proc/self/fd/") {
        if let Some(fd) = path.strip_prefix("/proc/self/fd/") {
            if fd == "0" || fd == "1" || fd == "2" {
                return true;
            }
        }
    }

    if path.starts_with("/proc/") && path.contains("/fd/") {
        if let Some(fd_part) = path.strip_prefix("/proc/") {
            if let Some(rest) = fd_part.find("/fd/") {
                let pid_and_rest = &fd_part[..rest];
                let fd = &fd_part[rest + 4..];
                // Verify the PID part is digits only (basic validation)
                if pid_and_rest.chars().all(|c| c.is_ascii_digit())
                    && (fd == "0" || fd == "1" || fd == "2")
                {
                    return true;
                }
            }
        }
    }

    false
}
