use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use super::manifest::LoadedDeclarativeTool;

#[derive(Clone)]
pub struct DeclarativeToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl DeclarativeToolSpec {
    pub fn from_loaded_tool(tool: &LoadedDeclarativeTool) -> Self {
        Self {
            id: ToolId(format!("plugin_declarative_{}", tool.manifest.name)),
            name: ToolName(tool.manifest.name.clone()),
            description: tool.manifest.description.clone(),
            input_schema: InputSchemaRef {
                schema: tool.input_schema_json.clone(),
            },
            output_contract: OutputContract {
                description: tool
                    .manifest
                    .output
                    .as_ref()
                    .map(|output| output.description.clone())
                    .unwrap_or_else(|| "Tool output".to_string()),
            },
            effect_profile: EffectProfile::from(&tool.manifest.effect),
        }
    }
}

impl ToolSpecView for DeclarativeToolSpec {
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
