use serde::{Deserialize, Serialize};

use crate::r#impl::lsp_hooks::LspDiagnosticsInfo;

/// Output types for FileWriteTool.
///
/// Matches the TypeScript outputSchema:
/// - create: New file creation
/// - update: Existing file modification

/// Hunk representation for diff output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// Starting line number in the old file.
    pub old_start: u32,
    /// Number of old lines in this hunk.
    pub old_lines: u32,
    /// Starting line number in the new file.
    pub new_start: u32,
    /// Number of new lines in this hunk.
    pub new_lines: u32,
    /// The lines in this hunk (diff text).
    pub lines: Vec<String>,
}

/// Structured patch representation for file changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredPatch {
    /// Optional hunks in this patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hunks: Option<Vec<Hunk>>,
}

/// Git diff information.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    /// Raw diff string from git.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// Whether the file has uncommitted changes.
    pub has_uncommitted_changes: bool,
}

/// Output for newly created files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOutput {
    /// The path to the file that was created.
    pub file_path: String,
    /// The content written to the file.
    pub content: String,
    /// Structured patch representation.
    pub structured_patch: StructuredPatch,
    /// Original file content (null for new files).
    pub original_file: serde_json::Value,
    /// Git diff information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<GitDiff>,
    /// LSP diagnostics for the file after the write (None if LSP unavailable or timed out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lsp_diagnostics: Option<LspDiagnosticsInfo>,
}

/// Output for updated/modified files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutput {
    /// The path to the file that was updated.
    pub file_path: String,
    /// The content written to the file.
    pub content: String,
    /// Structured patch representation.
    pub structured_patch: StructuredPatch,
    /// Original file content before update.
    pub original_file: String,
    /// Git diff information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<GitDiff>,
    /// LSP diagnostics for the file after the write (None if LSP unavailable or timed out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lsp_diagnostics: Option<LspDiagnosticsInfo>,
}

/// Enum representing all possible FileWriteTool outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum FileWriteOutput {
    /// New file creation.
    Create(CreateOutput),
    /// Existing file update.
    Update(UpdateOutput),
}
