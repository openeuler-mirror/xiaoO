use serde::{Deserialize, Serialize};

/// Input schema for FileReadTool.
///
/// Matches the TypeScript inputSchema:
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "file_path": { "type": "string", "description": "The absolute path to the file to read" },
///     "offset": { "type": "number", "description": "The line number to start reading from" },
///     "limit": { "type": "number", "description": "The number of lines to read" },
///     "pages": { "type": "string", "description": "Page range for PDF files" }
///   },
///   "required": ["file_path"]
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileReadInput {
    /// The absolute path to the file to read
    pub file_path: String,

    /// The line number to start reading from
    #[serde(default)]
    pub offset: Option<u64>,

    /// The number of lines to read
    #[serde(default)]
    pub limit: Option<u64>,

    /// Page range for PDF files (e.g., "1-5")
    #[serde(default)]
    pub pages: Option<String>,
}
