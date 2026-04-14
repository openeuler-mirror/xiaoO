use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Clone)]
pub struct CountTextLengthToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl CountTextLengthToolSpec {
    pub fn new() -> Self {
        Self {
            id: ToolId("builtin_count_text_length".to_string()),
            name: ToolName("count_text_length".to_string()),
            description: "Counts the number of characters in the input text".to_string(),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "The text to count characters for"
                        }
                    },
                    "required": ["text"]
                }),
            },
            output_contract: OutputContract {
                description: "A string containing the character count".to_string(),
            },
            effect_profile: EffectProfile::default(),
        }
    }
}

impl ToolSpecView for CountTextLengthToolSpec {
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
