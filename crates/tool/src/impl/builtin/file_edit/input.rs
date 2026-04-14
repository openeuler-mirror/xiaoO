use serde::{Deserialize, Serialize};

/// Input schema for FileEditTool.
///
/// Matches the TypeScript inputSchema:
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "file_path": { "type": "string", "description": "The absolute path to the file to edit" },
///     "old_string": { "type": "string", "description": "The string to search for" },
///     "new_string": { "type": "string", "description": "The string to replace with" },
///     "replace_all": { "type": "boolean", "description": "Replace all occurrences", "default": false }
///   },
///   "required": ["file_path", "old_string", "new_string"]
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileEditInput {
    /// The absolute path to the file to edit
    pub file_path: String,

    /// The string to search for
    pub old_string: String,

    /// The string to replace with
    pub new_string: String,

    /// Replace all occurrences of old_string
    #[serde(default)]
    pub replace_all: bool,
}
