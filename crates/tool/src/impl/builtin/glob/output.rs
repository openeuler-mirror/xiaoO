use serde::{Deserialize, Serialize};

/// Output type for GlobTool.
///
/// Matches the TypeScript outputSchema:
/// - durationMs: Time taken to execute in milliseconds
/// - numFiles: Total number of files found
/// - filenames: Array of matching file paths
/// - truncated: Whether results were limited

/// Output for GlobTool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobOutput {
    /// Time taken to execute in milliseconds.
    pub duration_ms: u64,
    /// Total number of files found.
    pub num_files: u64,
    /// Array of matching file paths.
    pub filenames: Vec<String>,
    /// Whether results were limited.
    pub truncated: bool,
    /// When `truncated`, how to retrieve the rest: the backend caps at a fixed
    /// limit and does not page, so the affordance is to narrow the query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_note: Option<String>,
}
