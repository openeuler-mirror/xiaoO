//! Landlock LSM filesystem access control.
//!
//! This module provides filesystem access control via Linux Landlock LSM.
//! Landlock allows fine-grained sandboxing of filesystem operations without
//! requiring root privileges.
//!
//! # Requirements
//!
//! - Linux kernel 5.13 or later
//! - Landlock must be enabled in the kernel configuration
//!
//! # Platform Support
//!
//! This module is only available on Linux. All functions return
//! `UnsupportedPlatform` errors on other platforms.

use crate::error::SandboxSetupError;
use crate::policy::{FsPermission, FsRule};

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

#[cfg(target_os = "linux")]
const REQUIRED_KERNEL_MAJOR: u32 = 5;
#[cfg(target_os = "linux")]
const REQUIRED_KERNEL_MINOR: u32 = 13;

#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;

#[cfg(target_os = "linux")]
const ALL_FS: u64 = LANDLOCK_ACCESS_FS_EXECUTE
    | LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_READ_FILE
    | LANDLOCK_ACCESS_FS_READ_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM;

#[cfg(target_os = "linux")]
const READ_EXECUTE: u64 =
    LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;

#[cfg(target_os = "linux")]
const READ_ONLY_DIR: u64 = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;

#[cfg(target_os = "linux")]
const READ_ONLY_FILE: u64 = LANDLOCK_ACCESS_FS_READ_FILE;

#[cfg(target_os = "linux")]
const READ_WRITE: u64 = LANDLOCK_ACCESS_FS_READ_FILE
    | LANDLOCK_ACCESS_FS_READ_DIR
    | LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SYM;

#[cfg(target_os = "linux")]
const READ_WRITE_EXECUTE: u64 = READ_WRITE | LANDLOCK_ACCESS_FS_EXECUTE;

#[cfg(target_os = "linux")]
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct PathBeneath {
    allowed_access: u64,
    parent_fd: i32,
    _pad: i32,
}

/// Check if Landlock is supported on the current kernel.
///
/// Returns `Ok(())` if Landlock is available and can be used,
/// or an error describing why it's not available.
#[cfg(target_os = "linux")]
pub fn check_landlock_support() -> Result<(), SandboxSetupError> {
    let current = read_kernel_version().map_err(|e| {
        SandboxSetupError::LandlockSetupFailed(format!("Failed to read kernel version: {}", e))
    })?;
    let (major, minor) = parse_kernel_version(&current).ok_or_else(|| {
        SandboxSetupError::LandlockSetupFailed(format!(
            "Unable to parse kernel version: {}",
            current
        ))
    })?;

    let supported = major > REQUIRED_KERNEL_MAJOR
        || (major == REQUIRED_KERNEL_MAJOR && minor >= REQUIRED_KERNEL_MINOR);

    if supported {
        Ok(())
    } else {
        Err(SandboxSetupError::LandlockSetupFailed(format!(
            "Kernel too old: current {}.{} required {}.{}",
            major, minor, REQUIRED_KERNEL_MAJOR, REQUIRED_KERNEL_MINOR
        )))
    }
}

/// Apply Landlock filesystem rules to restrict file access.
///
/// This function creates a Landlock ruleset with the provided filesystem
/// rules and applies it to the current process. After this call,
/// the process will only be able to access filesystem paths that match
/// the provided rules.
///
/// # Arguments
///
/// * `rules` - Slice of filesystem rules defining allowed access
///
/// # Errors
///
/// Returns `SandboxSetupError::LandlockSetupFailed` if:
/// - Landlock syscalls fail
/// - PR_SET_NO_NEW_PRIVS fails
#[cfg(target_os = "linux")]
pub fn apply_landlock_rules(rules: &[FsRule]) -> Result<(), SandboxSetupError> {
    let attr = RulesetAttr {
        handled_access_fs: ALL_FS,
        handled_access_net: 0,
    };

    // landlock_create_ruleset syscall (444)
    let fd = unsafe {
        libc::syscall(
            444,
            &attr as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            0_u64,
        )
    };

    if fd < 0 {
        return Err(SandboxSetupError::LandlockSetupFailed(format!(
            "landlock_create_ruleset failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let fd = fd as i32;

    for rule in rules {
        let file = match File::open(&rule.path) {
            Ok(f) => f,
            Err(_) => {
                // Skip paths that don't exist - this is expected for some policies
                continue;
            }
        };

        let is_dir = file.metadata().map(|m| m.is_dir()).unwrap_or(false);
        let access = map_permission(&rule.permission, is_dir);

        let path_beneath = PathBeneath {
            allowed_access: access,
            parent_fd: file.as_raw_fd(),
            _pad: 0,
        };

        // landlock_add_rule syscall (445)
        let result =
            unsafe { libc::syscall(445, fd, 1_u64, &path_beneath as *const PathBeneath, 0_u64) };

        if result < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINVAL) {
                unsafe { libc::close(fd) };
                return Err(SandboxSetupError::LandlockSetupFailed(format!(
                    "landlock_add_rule failed for {}: {}",
                    rule.path.display(),
                    err
                )));
            }
        }
    }

    // landlock_restrict_self syscall (446)
    let result = unsafe { libc::syscall(446, fd, 0_u64) };
    unsafe { libc::close(fd) };

    if result < 0 {
        return Err(SandboxSetupError::LandlockSetupFailed(format!(
            "landlock_restrict_self failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    // Set PR_SET_NO_NEW_PRIVS to ensure Landlock restrictions persist across execve
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if result < 0 {
        return Err(SandboxSetupError::LandlockSetupFailed(format!(
            "prctl(PR_SET_NO_NEW_PRIVS) failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn map_permission(permission: &FsPermission, is_dir: bool) -> u64 {
    match permission {
        FsPermission::ReadOnly => {
            if is_dir {
                READ_ONLY_DIR
            } else {
                READ_ONLY_FILE
            }
        }
        FsPermission::ReadWrite => {
            if is_dir {
                READ_WRITE
            } else {
                // Files can only receive READ_FILE and WRITE_FILE
                LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_WRITE_FILE
            }
        }
        FsPermission::ReadExecute => {
            if is_dir {
                READ_EXECUTE
            } else {
                // Files can receive EXECUTE and READ_FILE
                LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE
            }
        }
        FsPermission::ReadWriteExecute => {
            if is_dir {
                READ_WRITE_EXECUTE
            } else {
                LANDLOCK_ACCESS_FS_EXECUTE
                    | LANDLOCK_ACCESS_FS_READ_FILE
                    | LANDLOCK_ACCESS_FS_WRITE_FILE
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn read_kernel_version() -> Result<String, String> {
    match std::fs::read_to_string("/proc/version") {
        Ok(contents) => Ok(contents),
        Err(_) => std::process::Command::new("uname")
            .arg("-r")
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .map_err(|err| err.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn parse_kernel_version(source: &str) -> Option<(u32, u32)> {
    let token = source
        .split_whitespace()
        .find(|part| part.chars().any(|c| c.is_ascii_digit()) && part.contains('.'))?;
    let cleaned = token.split('-').next().unwrap_or(token);
    let mut parts = cleaned.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor))
}

// Non-Linux stubs

#[cfg(not(target_os = "linux"))]
pub fn check_landlock_support() -> Result<(), SandboxSetupError> {
    Err(SandboxSetupError::UnsupportedPlatform)
}

#[cfg(not(target_os = "linux"))]
pub fn apply_landlock_rules(_rules: &[FsRule]) -> Result<(), SandboxSetupError> {
    Err(SandboxSetupError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_fs_rule_creation() {
        let rule = FsRule {
            path: PathBuf::from("/tmp"),
            permission: FsPermission::ReadWrite,
        };
        assert_eq!(rule.path, PathBuf::from("/tmp"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_map_permission_supports_read_write_execute() {
        assert_eq!(
            map_permission(&FsPermission::ReadWriteExecute, false),
            LANDLOCK_ACCESS_FS_EXECUTE
                | LANDLOCK_ACCESS_FS_READ_FILE
                | LANDLOCK_ACCESS_FS_WRITE_FILE
        );
        assert_eq!(
            map_permission(&FsPermission::ReadWriteExecute, true),
            READ_WRITE_EXECUTE
        );
        assert_ne!(READ_WRITE_EXECUTE & LANDLOCK_ACCESS_FS_REMOVE_FILE, 0);
        assert_ne!(READ_WRITE_EXECUTE & LANDLOCK_ACCESS_FS_REMOVE_DIR, 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_read_write_directory_permission_supports_remove_operations() {
        let mapped = map_permission(&FsPermission::ReadWrite, true);

        assert_ne!(mapped & LANDLOCK_ACCESS_FS_REMOVE_FILE, 0);
        assert_ne!(mapped & LANDLOCK_ACCESS_FS_REMOVE_DIR, 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_parse_kernel_version() {
        // Test various kernel version formats
        assert_eq!(parse_kernel_version("5.15.0-generic"), Some((5, 15)));
        assert_eq!(parse_kernel_version("Linux version 5.10.0"), Some((5, 10)));
        assert_eq!(parse_kernel_version("6.1.0"), Some((6, 1)));
        assert_eq!(parse_kernel_version("invalid"), None);
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_unsupported_platform() {
        assert!(check_landlock_support().is_err());
        let rules = vec![];
        assert!(apply_landlock_rules(&rules).is_err());
    }
}
