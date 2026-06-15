//! Per-command process group management for xiaoo.
//!
//! This module provides utilities to manage process groups for each spawned command,
//! ensuring that all child and grandchild processes are terminated when:
//! - A command times out or is cancelled
//! - The main process exits (via Ctrl-C, SIGTERM, or normal shutdown)
//!
//! Design:
//! - Each spawned command creates its own process group via `process_group(0)`
//! - The process group ID (pgid) is registered in a global registry
//! - On timeout/cancel, `terminate_process_group()` kills the entire group
//! - On main process exit, `kill_all_process_groups()` cleans up all registered groups
//!
//! Cross-platform:
//! - Linux and macOS both support process groups via `setpgid()` and `killpg()`
//! - No platform-specific fallback needed (unlike PR_SET_PDEATHSIG which is Linux-only)

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(unix)]
use std::sync::LazyLock;

#[cfg(unix)]
static ACTIVE_PGIDS: LazyLock<Arc<Mutex<HashSet<i32>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashSet::new())));

/// Register a process group ID.
///
/// This should be called immediately after spawning a command with `process_group(0)`.
/// The pgid is typically equal to the child's PID.
#[cfg(unix)]
pub fn register_pgid(pgid: i32) {
    if pgid > 0 {
        if let Ok(mut set) = ACTIVE_PGIDS.lock() {
            set.insert(pgid);
            tracing::debug!("Registered process group: {}", pgid);
        }
    }
}

#[cfg(not(unix))]
pub fn register_pgid(_pgid: i32) {}

/// Unregister a process group ID.
///
/// This should be called after a process group has been fully terminated.
#[cfg(unix)]
pub fn unregister_pgid(pgid: i32) {
    if let Ok(mut set) = ACTIVE_PGIDS.lock() {
        set.remove(&pgid);
        tracing::debug!("Unregistered process group: {}", pgid);
    }
}

#[cfg(not(unix))]
pub fn unregister_pgid(_pgid: i32) {}

/// Terminate a specific process group (synchronous version).
///
/// Sends SIGTERM, waits 300ms, then sends SIGKILL to ensure all processes
/// in the group are terminated. This handles processes that spawn children
/// (e.g., `sh -c "sleep 1000 &"`).
///
/// # Arguments
///
/// * `pgid` - The process group ID (must be positive)
///
/// # Returns
///
/// Returns Ok(()) on success, or an error message on failure.
#[cfg(unix)]
pub fn terminate_process_group(pgid: i32) -> Result<(), String> {
    if pgid <= 0 {
        return Err(format!("Invalid process group ID: {}", pgid));
    }

    tracing::debug!("Terminating process group: {}", pgid);

    // Send SIGTERM to all processes in the group (negative PGID targets the group)
    let result = unsafe { libc::kill(-pgid, libc::SIGTERM) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH means no such process - group might already be empty
        if err.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!("Failed to send SIGTERM to process group {}: {}", pgid, err);
        }
    }

    // Wait for graceful termination
    std::thread::sleep(Duration::from_millis(300));

    // Send SIGKILL to any remaining processes
    let result = unsafe { libc::kill(-pgid, libc::SIGKILL) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!("Failed to send SIGKILL to process group {}: {}", pgid, err);
        }
    }

    unregister_pgid(pgid);
    Ok(())
}

#[cfg(not(unix))]
pub fn terminate_process_group(_pgid: i32) -> Result<(), String> {
    Err("Process groups are only supported on Unix systems".to_string())
}

/// Send SIGTERM to a process group without waiting.
///
/// This is the first step of termination. Follow up with `send_sigkill_to_group`
/// after a delay.
#[cfg(unix)]
pub fn send_sigterm_to_group(pgid: i32) {
    if pgid <= 0 {
        return;
    }
    let result = unsafe { libc::kill(-pgid, libc::SIGTERM) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!("Failed to send SIGTERM to process group {}: {}", pgid, err);
        }
    }
}

/// Send SIGKILL to a process group.
///
/// This should be called after SIGTERM and a delay to force-kill any remaining processes.
#[cfg(unix)]
pub fn send_sigkill_to_group(pgid: i32) {
    if pgid <= 0 {
        return;
    }
    let result = unsafe { libc::kill(-pgid, libc::SIGKILL) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!("Failed to send SIGKILL to process group {}: {}", pgid, err);
        }
    }
    unregister_pgid(pgid);
}

/// Kill all registered process groups.
///
/// This should be called on main process exit (Ctrl-C, SIGTERM, or normal shutdown)
/// to ensure all spawned processes are terminated.
///
/// Iterates through all registered process groups and terminates them.
#[cfg(unix)]
pub fn kill_all_process_groups() {
    let pgids: Vec<i32> = {
        if let Ok(mut set) = ACTIVE_PGIDS.lock() {
            let pgids: Vec<i32> = set.iter().cloned().collect();
            set.clear();
            pgids
        } else {
            return;
        }
    };

    if !pgids.is_empty() {
        tracing::info!("Killing {} active process group(s)", pgids.len());
    }

    for pgid in pgids {
        let _ = terminate_process_group(pgid);
    }
}

#[cfg(not(unix))]
pub fn kill_all_process_groups() {}

/// RAII guard for cleaning up all process groups on program exit.
///
/// This should be instantiated at the beginning of the main function.
/// When dropped (on program exit), it will call `kill_all_process_groups()`
/// to ensure all spawned processes are terminated.
#[cfg(unix)]
pub struct ProcessGroupCleanupGuard;

#[cfg(unix)]
impl Drop for ProcessGroupCleanupGuard {
    fn drop(&mut self) {
        kill_all_process_groups();
    }
}

#[cfg(not(unix))]
pub struct ProcessGroupCleanupGuard;

#[cfg(not(unix))]
impl Drop for ProcessGroupCleanupGuard {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn test_register_unregister_pgid() {
        register_pgid(12345);
        {
            if let Ok(set) = ACTIVE_PGIDS.lock() {
                assert!(set.contains(&12345));
            }
        }
        unregister_pgid(12345);
        {
            if let Ok(set) = ACTIVE_PGIDS.lock() {
                assert!(!set.contains(&12345));
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_kill_all_process_groups() {
        register_pgid(11111);
        register_pgid(22222);
        {
            if let Ok(set) = ACTIVE_PGIDS.lock() {
                assert_eq!(set.len(), 2);
            }
        }
        kill_all_process_groups();
        {
            if let Ok(set) = ACTIVE_PGIDS.lock() {
                assert!(set.is_empty());
            }
        }
    }

    #[test]
    fn test_terminate_invalid_pgid() {
        let result = terminate_process_group(-1);
        #[cfg(unix)]
        assert!(result.is_err());
    }
}
