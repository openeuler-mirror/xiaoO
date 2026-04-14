use serde::{Deserialize, Serialize};

/// Input schema for FileWriteTool.
///
/// Matches the TypeScript inputSchema:
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "file_path": { "type": "string", "description": "The absolute path to the file to write" },
///     "content": { "type": "string", "description": "The content to write to the file" }
///   },
///   "required": ["file_path", "content"]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileWriteInput {
    /// The absolute path to the file to write
    pub file_path: String,

    /// The content to write to the file
    pub content: String,
}
