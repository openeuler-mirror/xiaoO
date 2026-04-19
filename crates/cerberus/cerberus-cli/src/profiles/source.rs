//! Policy source model for layered profile resolution.
//!
//! Supports both built-in profiles and file-backed TOML policy files
//! with deterministic precedence:
//! 1. Explicit file path override
//! 2. Discovered config-backed profile
//! 3. Built-in profile fallback

use std::path::PathBuf;

/// Source of a resolved policy.
#[derive(Clone, Debug, PartialEq)]
pub enum PolicySource {
    /// Built-in named profile.
    BuiltIn {
        /// Profile name.
        name: String,
    },
    /// File-backed TOML policy.
    File {
        /// Profile name (derived from filename without .toml extension).
        name: String,
        /// Absolute path to the TOML file.
        path: PathBuf,
    },
}

impl PolicySource {
    /// Get the profile name.
    pub fn name(&self) -> &str {
        match self {
            PolicySource::BuiltIn { name } => name,
            PolicySource::File { name, .. } => name,
        }
    }

    /// Check if this is a built-in profile.
    pub fn is_builtin(&self) -> bool {
        matches!(self, PolicySource::BuiltIn { .. })
    }

    /// Check if this is a file-backed profile.
    pub fn is_file(&self) -> bool {
        matches!(self, PolicySource::File { .. })
    }

    /// Get the file path if this is a file-backed profile.
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            PolicySource::BuiltIn { .. } => None,
            PolicySource::File { path, .. } => Some(path),
        }
    }
}

impl std::fmt::Display for PolicySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicySource::BuiltIn { name } => write!(f, "built-in:{}", name),
            PolicySource::File { name, path } => {
                write!(f, "file:{} ({})", name, path.display())
            }
        }
    }
}
