use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct SpawnSubagentToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl SpawnSubagentToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short description of the delegated task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The full instruction for the subagent"
                }
            },
            "required": ["description", "prompt"]
        });

        Self {
            id: ToolId("builtin_spawn_subagent".to_string()),
            name: ToolName("spawn_subagent".to_string()),
            description: "Spawns an asynchronous subagent inside the current session".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Serialized JSON containing the spawned subagent agent_id".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: false,
                writes_filesystem: false,
                network_access: false,
                side_effects: true,
            },
        }
    }
}

impl Default for SpawnSubagentToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for SpawnSubagentToolSpec {
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
