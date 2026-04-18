//! Filesystem access control types.
//!
//! This module provides types for defining filesystem access rules
//! within sandboxed execution environments.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Filesystem permission levels.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FsPermission {
    /// Read-only access to files.
    ReadOnly,
    /// Read and write access to files.
    ReadWrite,
    /// Read and execute access to files.
    ReadExecute,
    /// Read, write, and execute access to files.
    ReadWriteExecute,
}

impl FsPermission {
    pub const fn allows_write(self) -> bool {
        matches!(self, Self::ReadWrite | Self::ReadWriteExecute)
    }

    pub const fn allows_execute(self) -> bool {
        matches!(self, Self::ReadExecute | Self::ReadWriteExecute)
    }
}

/// A single filesystem access rule.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FsRule {
    /// The path this rule applies to.
    pub path: PathBuf,
    /// The permission level for this path.
    pub permission: FsPermission,
}

/// Predefined groups of common filesystem paths.
///
/// PathGroups provides a convenient way to include commonly needed
/// filesystem paths in a policy, such as system binaries, libraries,
/// and temporary directories.
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PathGroups {
    /// Include /usr/bin (system executables).
    #[serde(default)]
    pub system_binaries: bool,
    /// Include /usr/lib, /lib, /lib64, /usr/share (shared libraries).
    #[serde(default)]
    pub system_libraries: bool,
    /// Include /tmp (temporary files directory).
    #[serde(default)]
    pub temp_directories: bool,
    /// Include /dev (device files).
    #[serde(default)]
    pub device_files: bool,
    /// Include /proc (process information filesystem).
    #[serde(default)]
    pub proc_filesystem: bool,
    /// Include /mnt/wsl (WSL-specific files).
    #[serde(default)]
    pub wsl_paths: bool,
}

impl PathGroups {
    pub fn strict() -> Self {
        Self {
            system_binaries: true,
            system_libraries: true,
            temp_directories: true,
            device_files: true,
            proc_filesystem: true,
            wsl_paths: false,
        }
    }

    /// Minimal preset: only basic paths needed for execution.
    pub fn minimal() -> Self {
        Self {
            system_binaries: true,
            system_libraries: true,
            temp_directories: true,
            device_files: false,
            proc_filesystem: false,
            wsl_paths: false,
        }
    }

    /// Convert enabled path groups into concrete filesystem rules.
    pub fn to_rules(&self) -> Vec<FsRule> {
        let mut rules = Vec::new();

        if self.system_binaries {
            rules.extend(Self::system_binary_rules());
        }

        if self.system_libraries {
            rules.extend(Self::system_library_rules());
        }

        if self.temp_directories {
            rules.extend(Self::temp_directory_rules());
        }

        if self.device_files {
            rules.extend(Self::device_file_rules());
        }

        if self.proc_filesystem {
            rules.extend(Self::proc_filesystem_rules());
        }

        if self.wsl_paths {
            rules.extend(Self::wsl_path_rules());
        }

        rules
    }

    fn system_binary_rules() -> Vec<FsRule> {
        vec![FsRule {
            path: PathBuf::from("/usr/bin"),
            permission: FsPermission::ReadExecute,
        }]
    }

    fn system_library_rules() -> Vec<FsRule> {
        vec![
            FsRule {
                path: PathBuf::from("/usr/lib"),
                permission: FsPermission::ReadExecute,
            },
            FsRule {
                path: PathBuf::from("/usr/lib64"),
                permission: FsPermission::ReadExecute,
            },
            FsRule {
                path: PathBuf::from("/lib"),
                permission: FsPermission::ReadExecute,
            },
            FsRule {
                path: PathBuf::from("/lib64"),
                permission: FsPermission::ReadExecute,
            },
            FsRule {
                path: PathBuf::from("/usr/share"),
                permission: FsPermission::ReadOnly,
            },
            FsRule {
                path: PathBuf::from("/etc/ld.so.cache"),
                permission: FsPermission::ReadOnly,
            },
            FsRule {
                path: PathBuf::from("/etc/localtime"),
                permission: FsPermission::ReadOnly,
            },
            FsRule {
                path: PathBuf::from("/etc/locale.alias"),
                permission: FsPermission::ReadOnly,
            },
        ]
    }

    fn temp_directory_rules() -> Vec<FsRule> {
        vec![FsRule {
            path: PathBuf::from("/tmp"),
            permission: FsPermission::ReadWrite,
        }]
    }

    fn device_file_rules() -> Vec<FsRule> {
        vec![FsRule {
            path: PathBuf::from("/dev"),
            permission: FsPermission::ReadWrite,
        }]
    }

    fn proc_filesystem_rules() -> Vec<FsRule> {
        vec![FsRule {
            path: PathBuf::from("/proc"),
            permission: FsPermission::ReadOnly,
        }]
    }

    fn wsl_path_rules() -> Vec<FsRule> {
        vec![FsRule {
            path: PathBuf::from("/mnt/wsl"),
            permission: FsPermission::ReadOnly,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_groups_to_rules() {
        let groups = PathGroups::strict();
        let rules = groups.to_rules();
        assert!(!rules.is_empty());

        let paths: Vec<&str> = rules.iter().map(|r| r.path.to_str().unwrap()).collect();
        assert!(paths.contains(&"/usr/bin"));
        assert!(paths.contains(&"/tmp"));
        assert!(!paths.contains(&"/etc"));
    }

    #[test]
    fn test_path_groups_with_wsl_paths() {
        let mut groups = PathGroups::minimal();
        groups.wsl_paths = true;
        let rules = groups.to_rules();
        let paths: Vec<&str> = rules.iter().map(|r| r.path.to_str().unwrap()).collect();
        assert!(paths.contains(&"/mnt/wsl"));
    }
}
