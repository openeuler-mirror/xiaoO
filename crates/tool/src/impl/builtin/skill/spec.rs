use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct SkillToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl SkillToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name to invoke"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill"
                }
            },
            "required": ["skill"]
        });

        Self {
            id: ToolId("builtin_skill".to_string()),
            name: ToolName("skill".to_string()),
            description: "Invoke a registered skill by name. Skills are prompt-based capabilities that guide the agent to perform specific tasks.".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "The expanded skill prompt text, or an error message if the skill was not found or cannot be invoked.".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: false,
                writes_filesystem: false,
                network_access: false,
                side_effects: false,
            },
        }
    }
}

impl Default for SkillToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for SkillToolSpec {
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
