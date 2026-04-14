use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use agent_contracts::tool::spec::ToolSpecView;

#[derive(Debug, Clone)]
pub struct FileReadToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl FileReadToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "The absolute path to the file to read" },
                "offset": { "type": "number", "description": "The line number to start reading from" },
                "limit": { "type": "number", "description": "The number of lines to read" },
                "pages": { "type": "string", "description": "Page range for PDF files" }
            },
            "required": ["file_path"]
        });

        Self {
            id: ToolId("builtin_file_read".to_string()),
            name: ToolName("file_read".to_string()),
            description: "Reads files, images, PDFs, and notebooks".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Output from FileReadTool".to_string(),
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

impl Default for FileReadToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for FileReadToolSpec {
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
