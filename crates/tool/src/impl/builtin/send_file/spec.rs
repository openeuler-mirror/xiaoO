use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct SendFileToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl SendFileToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to send to the user"
                },
                "label": {
                    "type": "string",
                    "description": "Optional label or description for the file"
                }
            },
            "required": ["file_path"]
        });

        Self {
            id: ToolId("builtin_send_file".to_string()),
            name: ToolName("send_file".to_string()),
            description: "Send a file to the user in the current channel conversation. Only available in channel sessions (e.g. Feishu). The file will be uploaded and delivered to the user.".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Success message with the sent file path, or an error message.".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: false,
                network_access: true,
                side_effects: true,
            },
        }
    }
}

impl Default for SendFileToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for SendFileToolSpec {
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
