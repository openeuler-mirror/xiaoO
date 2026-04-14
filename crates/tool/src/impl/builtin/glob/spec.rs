use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use agent_contracts::tool::ToolSpecView;

#[derive(Debug, Clone)]
pub struct GlobToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl GlobToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against (e.g., '**/*.js' or 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "The root directory to search from (defaults to current working directory)"
                }
            },
            "required": ["pattern"]
        });

        Self {
            id: ToolId("builtin_glob".to_string()),
            name: ToolName("glob".to_string()),
            description: "- Fast file pattern matching tool that works with any codebase size\n- Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\"\n- Returns matching file paths sorted by modification time\n- Use this tool when you need to find files by name patterns\n- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Agent tool instead".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Output from GlobTool".to_string(),
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

impl ToolSpecView for GlobToolSpec {
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
