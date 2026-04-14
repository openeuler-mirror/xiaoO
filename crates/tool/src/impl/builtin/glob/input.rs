use serde::{Deserialize, Serialize};

/// Input schema for GlobTool.
///
/// Matches the TypeScript inputSchema:
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "pattern": { "type": "string", "description": "The glob pattern to match files against" },
///     "path": { "type": "string", "description": "The directory to search in" }
///   },
///   "required": ["pattern"]
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GlobInput {
    /// The glob pattern to match files against (e.g., "**/*.rs")
    pub pattern: String,

    /// The directory to search in (defaults to cwd)
    #[serde(default)]
    pub path: Option<String>,
}
