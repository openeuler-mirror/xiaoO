use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use agent_contracts::tool::spec::ToolSpecView;

use super::constants::{FILE_EDIT_TOOL_ID, FILE_EDIT_TOOL_NAME};

#[derive(Debug, Clone)]
pub struct FileEditToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl FileEditToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "The absolute path to the file to edit" },
                "old_string": { "type": "string", "description": "The exact string to find in the file" },
                "new_string": { "type": "string", "description": "The replacement string" },
                "replace_all": { "type": "boolean", "description": "Replace all occurrences or just the first" }
            },
            "required": ["file_path", "old_string", "new_string"]
        });

        Self {
            id: ToolId(FILE_EDIT_TOOL_ID.to_string()),
            name: ToolName(FILE_EDIT_TOOL_NAME.to_string()),
            description: "Edits files by replacing exact strings".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Output from FileEditTool".to_string(),
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

impl Default for FileEditToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for FileEditToolSpec {
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
