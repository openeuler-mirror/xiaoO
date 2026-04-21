use serde::{Deserialize, Serialize};

use crate::r#impl::lsp_hooks::LspDiagnosticsInfo;

/// Output types for FileEditTool.
///
/// Matches the TypeScript outputSchema:
/// - Hunk: Individual patch hunk with line range information
/// - StructuredPatch: Array of hunks representing a complete patch
/// - GitDiff: Git diff metadata (filename, status, additions, deletions, etc.)
/// - FileEditOutput: Complete output of a file edit operation

/// A hunk in a structured patch.
///
/// Represents a contiguous block of changes with line range information
/// for both the original and modified versions of the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// Starting line number in the original file (1-indexed).
    pub old_start: u32,
    /// Number of lines in the original file for this hunk.
    pub old_lines: u32,
    /// Starting line number in the modified file (1-indexed).
    pub new_start: u32,
    /// Number of lines in the modified file for this hunk.
    pub new_lines: u32,
    /// The actual content lines of this hunk.
    pub lines: Vec<String>,
}

/// A structured patch composed of multiple hunks.
///
/// This is a type alias for a vector of hunks, representing
/// the complete set of changes to a file.
pub type StructuredPatch = Vec<Hunk>;

/// Git diff metadata for a file edit operation.
///
/// Contains information about the diff statistics and the raw patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    /// The filename that was modified.
    pub filename: String,
    /// The status of the file (e.g., "modified", "added", "deleted").
    pub status: String,
    /// Number of lines added.
    pub additions: u32,
    /// Number of lines deleted.
    pub deletions: u32,
    /// Total number of changes (additions + deletions).
    pub changes: u32,
    /// The raw unified diff patch string.
    pub patch: String,
    /// The repository path (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

/// Complete output from a FileEditTool operation.
///
/// Represents all information about a file edit, including the original
/// and new content, the structured patch, and optional git diff metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEditOutput {
    /// The path to the file that was edited.
    pub file_path: String,
    /// The original string content that was replaced.
    pub old_string: String,
    /// The new string content that replaced the old content.
    pub new_string: String,
    /// The original file content before any edits.
    pub original_file: String,
    /// The structured patch representing the changes made.
    pub structured_patch: StructuredPatch,
    /// Whether the file was modified by user action.
    pub user_modified: bool,
    /// Whether this was a replace-all operation.
    pub replace_all: bool,
    /// Git diff metadata for the edit (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<GitDiff>,
    /// LSP diagnostics for the file after the edit (None if LSP unavailable or timed out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lsp_diagnostics: Option<LspDiagnosticsInfo>,
}
