//! Constants for the builtin `grep` tool.
//!
//! - `DEFAULT_HEAD_LIMIT` caps result entries when the caller omits `head_limit`.

/// Default result cap applied when `head_limit` is omitted.
pub const DEFAULT_HEAD_LIMIT: u32 = 250;

pub const RG_MAX_COLUMNS: u32 = 500;

pub const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];
