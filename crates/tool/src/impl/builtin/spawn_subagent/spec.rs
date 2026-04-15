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
                    "description": "A short, concise description of the delegated task"
                },
                "task_goal": {
                    "type": "string",
                    "description": "The exact core goal the subagent needs to accomplish"
                },
                "task_context": {
                    "type": "string",
                    "description": "Any necessary contextual information to perform the task"
                },
                "output_schema": {
                    "type": "object",
                    "description": "The strict JSON schema that the subagent MUST follow when returning its final result"
                }
            },
            "required": ["description", "task_goal", "task_context", "output_schema"]
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
