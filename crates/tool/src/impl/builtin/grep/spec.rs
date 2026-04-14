use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct GrepToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl GrepToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for in file contents"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in (rg PATH). Defaults to current working directory."
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\") - maps to rg --glob"
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode: \"content\" shows matching lines, \"files_with_matches\" shows file paths, \"count\" shows match counts. Defaults to \"files_with_matches\"."
                },
                "-B": {
                    "type": "number",
                    "description": "Number of lines to show before each match (rg -B). Requires output_mode: \"content\"."
                },
                "-A": {
                    "type": "number",
                    "description": "Number of lines to show after each match (rg -A). Requires output_mode: \"content\"."
                },
                "-C": {
                    "type": "number",
                    "description": "Alias for context."
                },
                "context": {
                    "type": "number",
                    "description": "Number of lines to show before and after each match (rg -C). Requires output_mode: \"content\"."
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers in output (rg -n). Requires output_mode: \"content\". Defaults to true."
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search (rg -i)"
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (rg --type). Common types: js, py, rust, go, java, etc."
                },
                "head_limit": {
                    "type": "number",
                    "description": "Limit output to first N lines/entries. Defaults to 250 when unspecified. Pass 0 for unlimited."
                },
                "offset": {
                    "type": "number",
                    "description": "Skip first N lines/entries before applying head_limit. Defaults to 0."
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline mode where . matches newlines (rg -U --multiline-dotall). Default: false."
                }
            },
            "required": ["pattern"]
        });

        Self {
            id: ToolId("builtin_grep".to_string()),
            name: ToolName("grep".to_string()),
            description: "Search file contents using ripgrep (rg)".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Output from GrepTool".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: false,
                network_access: false,
                side_effects: false,
            },
        }
    }
}

impl Default for GrepToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for GrepToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}
