use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use super::constants::{default_timeout_ms, max_timeout_ms};
use agent_contracts::tool::ToolSpecView;

#[derive(Debug, Clone)]
pub struct BashToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl BashToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                },
                "timeout": {
                    "type": "number",
                    "description": format!(
                        "Optional timeout in milliseconds. Defaults to {}ms and may not exceed {}ms",
                        default_timeout_ms(),
                        max_timeout_ms()
                    )
                }
            },
            "required": ["command"]
        });

        Self {
            id: ToolId("builtin_bash".to_string()),
            name: ToolName("bash".to_string()),
            description: "Executes a bash command and returns structured process output"
                .to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description:
                    "Output from bash including stdout, stderr, exit_code, and interruption state"
                        .to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: true,
                network_access: false,
                side_effects: true,
            },
        }
    }
}

impl Default for BashToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for BashToolSpec {
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
