use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use agent_contracts::tool::spec::ToolSpecView;

use super::super::FILE_PATH_INPUT_KEY;
use super::constants::{FILE_WRITE_TOOL_ID, FILE_WRITE_TOOL_NAME};

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
                FILE_PATH_INPUT_KEY: { "type": "string", "description": "The absolute path to the file to write" },
                "content": { "type": "string", "description": "The content to write to the file" }
            },
            "required": [FILE_PATH_INPUT_KEY, "content"]
        });

        Self {
            id: ToolId(FILE_WRITE_TOOL_ID.to_string()),
            name: ToolName(FILE_WRITE_TOOL_NAME.to_string()),
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
