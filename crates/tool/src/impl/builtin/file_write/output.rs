//! Output types for FileWriteTool.
//!
//! Matches the TypeScript outputSchema:
//! - create: New file creation
//! - update: Existing file modification

use serde::{Deserialize, Serialize};

use crate::r#impl::lsp_hooks::LspDiagnosticsInfo;

/// Hunk representation for diff output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<String>,
}

/// Structured patch representation for file changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hunks: Option<Vec<Hunk>>,
}

/// Output for newly created files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOutput {
    pub file_path: String,
    pub new_lines: u32,
    pub structured_patch: StructuredPatch,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lsp_diagnostics: Option<LspDiagnosticsInfo>,
}

/// Output for updated/modified files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutput {
    pub file_path: String,
    pub new_lines: u32,
    pub structured_patch: StructuredPatch,
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
