/// Stable builtin tool id exposed through discovery.
pub const FILE_EDIT_TOOL_ID: &str = "builtin_file_edit";

/// Stable builtin tool name exposed through discovery.
pub const FILE_EDIT_TOOL_NAME: &str = "file_edit";

/// Maximum editable file size in bytes.
///
/// Validation rejects larger files before attempting an in-place edit.
/// Keep any user-facing documentation in sync with this limit.
pub const MAX_EDIT_FILE_SIZE: u64 = 1024 * 1024 * 1024;
