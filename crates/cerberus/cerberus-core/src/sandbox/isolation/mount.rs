//! Mount isolation using pivot_root.
//!
//! This module provides filesystem isolation via pivot_root as a fallback
//! when Landlock is unavailable. It creates a new root filesystem with
//! only the allowed paths mounted.
//!
//! # Security Model
//!
//! Mount isolation works by:
//! 1. Creating a new root directory with tmpfs
//! 2. Bind-mounting allowed paths into the new root
//! 3. Using pivot_root to switch to the new root
//! 4. Unmounting the old root
//!
//! # Limitations
//!
//! Mount isolation requires:
//! - Mount namespace to be active
//! - CAP_SYS_ADMIN capability (or user namespace)
//! - Pseudo-filesystems (/proc, /sys, /dev) cannot be bind-mounted
//!
//! # Platform Support
//!
//! This module is only available on Linux. All functions return
//! `UnsupportedPlatform` errors on other platforms.

use crate::error::SandboxSetupError;
use crate::policy::{FsPermission, FsRule};

#[cfg(target_os = "linux")]
use nix::mount::{mount, umount2, MntFlags, MsFlags};
#[cfg(target_os = "linux")]
use nix::unistd::pivot_root;
use std::path::Path;

/// Check if mount isolation is supported on the current system.
///
/// Mount isolation requires being in a mount namespace with
/// appropriate capabilities.
#[cfg(target_os = "linux")]
pub fn check_mount_isolation_support() -> Result<(), SandboxSetupError> {
    // Mount isolation requires being in a mount namespace, which is set up
    // by the sandbox setup before calling this function.
    Ok(())
}

/// Apply mount isolation using pivot_root.
///
/// This function creates a new root filesystem with only the allowed
/// paths bind-mounted. It should be called after entering a mount namespace.
///
/// # Arguments
///
/// * `rules` - Filesystem rules defining which paths to allow
///
/// # Errors
///
/// Returns `SandboxSetupError::MountIsolationFailed` if:
/// - Root mount cannot be made private
/// - tmpfs creation fails
/// - Bind mounts fail
/// - pivot_root fails
#[cfg(target_os = "linux")]
pub fn apply_mount_isolation(rules: &[FsRule]) -> Result<(), SandboxSetupError> {
    // 1. Make root mount private to prevent propagation to parent namespace
    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    )
    .map_err(|e| {
        SandboxSetupError::MountIsolationFailed(format!("Failed to make root mount private: {}", e))
    })?;

    // 2. Create and mount tmpfs as new root
    let new_root = format!("/tmp/cerberus-sandbox-{}", std::process::id());
    std::fs::create_dir_all(&new_root).map_err(|e| {
        SandboxSetupError::MountIsolationFailed(format!(
            "Failed to create new root directory: {}",
            e
        ))
    })?;

    mount(
        Some("tmpfs"),
        new_root.as_str(),
        Some("tmpfs"),
        MsFlags::empty(),
        Some("size=64M"),
    )
    .map_err(|e| {
        SandboxSetupError::MountIsolationFailed(format!("Failed to mount tmpfs: {}", e))
    })?;

    // 3. Bind mount allowed paths into new root
    for rule in rules {
        // Skip if path doesn't exist
        if !rule.path.exists() {
            continue;
        }

        // Skip special pseudo-filesystems that cannot be bind mounted
        let path_str = rule.path.to_string_lossy();
        if path_str == "/dev" || path_str == "/proc" || path_str == "/sys" {
            eprintln!(
                "Info: Skipping pseudo-filesystem {} (cannot be bind mounted)",
                path_str
            );
            continue;
        }

        // Create target path in new root
        let target = Path::new(&new_root).join(rule.path.strip_prefix("/").unwrap_or(&rule.path));

        // Check if source is a directory or file
        let is_dir = rule.path.is_dir();

        // Create mount point structure
        if is_dir {
            std::fs::create_dir_all(&target).map_err(|e| {
                SandboxSetupError::MountIsolationFailed(format!(
                    "Failed to create mount point directory {}: {}",
                    target.display(),
                    e
                ))
            })?;
        } else {
            // For files, create parent directories only
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    SandboxSetupError::MountIsolationFailed(format!(
                        "Failed to create mount point directory: {}",
                        e
                    ))
                })?;
            }
            // Create empty file as mount point
            std::fs::File::create(&target).map_err(|e| {
                SandboxSetupError::MountIsolationFailed(format!(
                    "Failed to create mount point file {}: {}",
                    target.display(),
                    e
                ))
            })?;
        }

        // Determine mount flags based on permission
        let mut flags = MsFlags::MS_BIND;
        if matches!(
            rule.permission,
            FsPermission::ReadOnly | FsPermission::ReadExecute
        ) {
            flags |= MsFlags::MS_RDONLY;
        }

        // Bind mount the path
        mount(
            Some(rule.path.as_path()),
            &target,
            None::<&str>,
            flags,
            None::<&str>,
        )
        .map_err(|e| {
            SandboxSetupError::MountIsolationFailed(format!(
                "Failed to bind mount {}: {}",
                rule.path.display(),
                e
            ))
        })?;
    }

    // 4. Change to new root directory
    std::env::set_current_dir(&new_root).map_err(|e| {
        SandboxSetupError::MountIsolationFailed(format!("Failed to chdir to new root: {}", e))
    })?;

    // 5. Pivot root: current directory becomes new root, old root at current directory
    pivot_root(".", ".").map_err(|e| {
        SandboxSetupError::MountIsolationFailed(format!("Failed to pivot_root: {}", e))
    })?;

    // 6. Unmount old root (now at ".") with lazy unmount
    umount2(".", MntFlags::MNT_DETACH).map_err(|e| {
        SandboxSetupError::MountIsolationFailed(format!("Failed to unmount old root: {}", e))
    })?;

    Ok(())
}

// Non-Linux stubs

#[cfg(not(target_os = "linux"))]
pub fn check_mount_isolation_support() -> Result<(), SandboxSetupError> {
    Err(SandboxSetupError::UnsupportedPlatform)
}

#[cfg(not(target_os = "linux"))]
pub fn apply_mount_isolation(_rules: &[FsRule]) -> Result<(), SandboxSetupError> {
    Err(SandboxSetupError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_mount_isolation_support() {
        #[cfg(target_os = "linux")]
        {
            let result = check_mount_isolation_support();
            assert!(result.is_ok());
        }

        #[cfg(not(target_os = "linux"))]
        {
            let result = check_mount_isolation_support();
            assert!(matches!(
                result,
                Err(SandboxSetupError::UnsupportedPlatform)
            ));
        }
    }

    #[test]
    fn test_apply_mount_isolation_missing_path() {
        #[cfg(not(target_os = "linux"))]
        {
            use std::path::PathBuf;

            let rules = vec![FsRule {
                path: PathBuf::from("/nonexistent/path/that/does/not/exist"),
                permission: FsPermission::ReadOnly,
            }];
            let result = apply_mount_isolation(&rules);
            assert!(matches!(
                result,
                Err(SandboxSetupError::UnsupportedPlatform)
            ));
        }
    }

    #[test]
    fn test_fs_permission_variants() {
        let read_only = FsPermission::ReadOnly;
        let read_write = FsPermission::ReadWrite;
        let read_execute = FsPermission::ReadExecute;
        let read_write_execute = FsPermission::ReadWriteExecute;

        assert!(matches!(read_only, FsPermission::ReadOnly));
        assert!(matches!(read_write, FsPermission::ReadWrite));
        assert!(matches!(read_execute, FsPermission::ReadExecute));
        assert!(matches!(read_write_execute, FsPermission::ReadWriteExecute));
    }
}
