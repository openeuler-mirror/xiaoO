use serde::{Deserialize, Serialize};

/// Output mode for GrepTool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Show matching lines with context
    Content,
    /// Show file paths that contain matches
    FilesWithMatches,
    /// Show match counts per file
    Count,
}

impl Default for OutputMode {
    fn default() -> Self {
        Self::FilesWithMatches
    }
}

/// Input schema for GrepTool.
///
/// Matches the TypeScript inputSchema:
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "pattern": { "type": "string", "description": "The regular expression pattern to search for" },
///     "path": { "type": "string", "description": "File or directory to search in" },
///     "glob": { "type": "string", "description": "Glob pattern to filter files" },
///     "output_mode": { "enum": ["content", "files_with_matches", "count"] },
///     "-B": { "type": "number", "description": "Lines before match" },
///     "-A": { "type": "number", "description": "Lines after match" },
///     "-C": { "type": "number", "description": "Context lines" },
///     "context": { "type": "number", "description": "Context lines before and after" },
///     "-n": { "type": "boolean", "description": "Show line numbers" },
///     "-i": { "type": "boolean", "description": "Case insensitive" },
///     "type": { "type": "string", "description": "File type filter" },
///     "head_limit": { "type": "number", "description": "Limit output" },
///     "offset": { "type": "number", "description": "Skip first N results" },
///     "multiline": { "type": "boolean", "description": "Enable multiline mode" }
///   },
///   "required": ["pattern"]
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GrepInput {
    /// The regular expression pattern to search for in file contents (required)
    pub pattern: String,

    /// File or directory to search in (rg PATH). Defaults to current working directory.
    #[serde(default)]
    pub path: Option<String>,

    /// Glob pattern to filter files (e.g. "*.js", "*.{ts,tsx}") - maps to rg --glob
    #[serde(default)]
    pub glob: Option<String>,

    /// Output mode: "content" shows matching lines, "files_with_matches" shows file paths,
    /// "count" shows match counts. Defaults to "files_with_matches".
    #[serde(default)]
    pub output_mode: Option<OutputMode>,

    /// Number of lines to show before each match (rg -B)
    #[serde(default, alias = "-B")]
    pub context_before: Option<u32>,

    /// Number of lines to show after each match (rg -A)
    #[serde(default, alias = "-A")]
    pub context_after: Option<u32>,

    /// Alias for context (-C)
    #[serde(default, alias = "-C")]
    pub context_c: Option<u32>,

    /// Number of lines to show before and after each match (rg -C)
    #[serde(default)]
    pub context: Option<u32>,

    /// Show line numbers in output (rg -n). Requires output_mode: "content". Defaults to true.
    #[serde(default, alias = "-n")]
    pub show_line_numbers: Option<bool>,

    /// Case insensitive search (rg -i)
    #[serde(default, alias = "-i")]
    pub case_insensitive: Option<bool>,

    /// File type to search (rg --type). Common types: js, py, rust, go, java, etc.
    #[serde(default, alias = "type")]
    pub file_type: Option<String>,

    /// Limit output to first N lines/entries. Defaults to 250 when unspecified. Pass 0 for unlimited.
    #[serde(default)]
    pub head_limit: Option<u32>,

    /// Skip first N lines/entries before applying head_limit. Defaults to 0.
    #[serde(default)]
    pub offset: Option<u32>,

    /// Enable multiline mode where . matches newlines (rg -U --multiline-dotall)
    #[serde(default)]
    pub multiline: Option<bool>,
}
