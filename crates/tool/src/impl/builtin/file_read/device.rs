//! Device file path blocking for file read operations.
//!
//! This module provides path validation to prevent reading from device files
//! that could produce infinite output or other problematic behavior.

use super::constants::{BLOCKED_DEVICE_PATHS, BLOCKED_PROC_FILES};

/// Checks if a path refers to a blocked device file.
///
/// # Examples
/// ```
/// use tool::r#impl::file_read::device::is_blocked_device_path;
///
/// assert!(is_blocked_device_path("/dev/zero"));
/// assert!(is_blocked_device_path("/dev/urandom"));
/// assert!(is_blocked_device_path("/proc/self/fd/0"));
/// assert!(is_blocked_device_path("/proc/self/environ"));
/// assert!(!is_blocked_device_path("/dev/null")); // /dev/null is safe
/// assert!(!is_blocked_device_path("/etc/passwd"));
/// ```
pub fn is_blocked_device_path(path: &str) -> bool {
    if BLOCKED_DEVICE_PATHS.contains(&path) {
        return true;
    }

    if let Some(rest) = path.strip_prefix("/proc/") {
        if let Some(index) = rest.find("/fd/") {
            let owner = &rest[..index];
            let fd = &rest[index + "/fd/".len()..];
            if matches!(fd, "0" | "1" | "2") && is_process_fd_owner(owner) {
                return true;
            }
        }

        // Process-scoped files: `/proc/<pid>/environ`, `/proc/self/mem`, and the
        // per-thread form `/proc/<pid>/task/<tid>/cmdline`. Matched on the last
        // segment so every owner variant is covered by one rule.
        let file = rest.rsplit('/').next().unwrap_or("");
        if BLOCKED_PROC_FILES.contains(&file) {
            return true;
        }
    }

    false
}

/// Whether the path segment before `/fd/` identifies a process or thread fd table.
///
/// Besides `/proc/<pid>`, Linux exposes the same standard streams through
/// `/proc/self`, `/proc/thread-self` and the per-thread form `/proc/<pid>/task/<tid>`.
/// Reading any of those std fds blocks until the process receives input, which is
/// what the block list is meant to prevent.
fn is_process_fd_owner(owner: &str) -> bool {
    if owner == "self" || owner == "thread-self" {
        return true;
    }

    if let Some((head, tid)) = owner.split_once("/task/") {
        return (head == "self" || head == "thread-self" || is_pid(head)) && is_pid(tid);
    }

    is_pid(owner)
}

fn is_pid(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/tool/impl/builtin/file_read/device_test.rs"]
mod tests;
