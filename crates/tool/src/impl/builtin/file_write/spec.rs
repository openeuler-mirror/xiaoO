use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use agent_contracts::tool::spec::ToolSpecView;

#[derive(Debug, Clone)]
pub struct FileWriteToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl FileWriteToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "The absolute path to the file to write" },
                "content": { "type": "string", "description": "The content to write to the file" }
            },
            "required": ["file_path", "content"]
        });

        Self {
            id: ToolId("builtin_file_write".to_string()),
            name: ToolName("file_write".to_string()),
            description: "Writes content to a file at the specified path".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Output from FileWriteTool".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: true,
                network_access: false,
                side_effects: false,
            },
        }
    }
}

impl Default for FileWriteToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for FileWriteToolSpec {
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
