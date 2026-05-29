use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use std::collections::BTreeMap;

use crate::r#impl::SubagentRoleInfo;

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
        Self::with_subagent_roles(BTreeMap::new())
    }

    pub fn with_subagent_roles(subagent_roles: BTreeMap<String, SubagentRoleInfo>) -> Self {
        let roles_section = if subagent_roles.is_empty() {
            String::new()
        } else {
            let roles_list = subagent_roles
                .values()
                .map(|role| format!("- \"{}\": {}", role.role_id, role.description))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\nAvailable predefined subagent roles:\n{}", roles_list)
        };

        let description = format!(
            "Spawns an asynchronous subagent inside the current session. Use it when the request explicitly asks for subagents or parallel work, or when the work cleanly splits into multiple independent read-only branches whose results will later be compared, sorted, or aggregated. Do not use it for tiny single-step lookups, and do not attempt nested delegation from inside an already delegated subtask. When subagent_role_id is provided, the subagent uses a predefined role with fixed prompt and permissions from config.{}",
            roles_section
        );

        let roles_field = if subagent_roles.is_empty() {
            String::new()
        } else {
            let roles_list = subagent_roles
                .values()
                .map(|role| format!("- \"{}\": {}", role.role_id, role.description))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\nCurrently available roles:\n{}", roles_list)
        };

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short label for the delegated branch or subtask",
                    "examples": ["Calculate total token usage"]
                },
                "subagent_role_id": {
                    "type": "string",
                    "description": format!("Optional predefined subagent role ID from config. When specified, the subagent uses the role's fixed prompt, tool permissions, and max_turns. If not specified, a dynamic subagent is created using task_goal and task_context.{}", roles_field)
                },
                "task_goal": {
                    "type": "string",
                    "description": "The exact core goal the subagent needs to accomplish. When the task needs a count, comparison, or directory statistic, explicitly require an exact result and forbid approximate or truncated answers."
                },
                "task_context": {
                    "type": "string",
                    "description": "Any necessary contextual information to perform the task"
                },
                "output_schema": {
                    "type": "object",
                    "description": "The strict JSON schema that the subagent MUST follow when returning its final result. MUST be a valid JSON Schema object.",
                    "examples": [{
                        "type": "object",
                        "properties": {
                            "count": {
                                "type": "number",
                                "description": "The total count"
                            }
                        },
                        "required": ["count"]
                    }]
                }
            },
            "required": ["description", "task_goal", "task_context"]
        });

        Self {
            id: ToolId("builtin_spawn_subagent".to_string()),
            name: ToolName("spawn_subagent".to_string()),
            description,
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
