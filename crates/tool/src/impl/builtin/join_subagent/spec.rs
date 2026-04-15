use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct JoinSubagentToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl JoinSubagentToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "target_agent_id": {
                    "type": "string",
                    "description": "The subagent id to wait for"
                }
            },
            "required": ["target_agent_id"]
        });

        Self {
            id: ToolId("builtin_join_subagent".to_string()),
            name: ToolName("join_subagent".to_string()),
            description: "Waits for a spawned subagent to finish within the current session. After joining, verify that the returned result is exact before you aggregate, compare, sort, or total multiple branches."
                .to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Serialized JSON containing the target subagent terminal snapshot"
                    .to_string(),
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

impl Default for JoinSubagentToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for JoinSubagentToolSpec {
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
