//! Constants for the builtin `grep` tool.
//!
//! - `DEFAULT_HEAD_LIMIT` caps result entries when the caller omits `head_limit`.
//! - `RG_MAX_COLUMNS` defines the value passed to `rg --max-columns`.
//! - `VCS_DIRECTORIES_TO_EXCLUDE` lists repository metadata directories hidden from search.

/// Default result cap applied when `head_limit` is omitted.
pub const DEFAULT_HEAD_LIMIT: u32 = 250;

/// Maximum line width forwarded to `rg --max-columns`.
pub const RG_MAX_COLUMNS: u32 = 500;

/// VCS metadata directories excluded from ripgrep traversal by default.
pub const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];
