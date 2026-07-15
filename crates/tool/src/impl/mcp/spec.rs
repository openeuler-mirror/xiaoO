use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use std::sync::Arc;

use super::tool_source::McpDiscoveredTool;

/// Spec for a single MCP-exposed tool. The name is namespaced as
/// `mcp__{server}__{tool}` to avoid collisions with builtins.
pub struct McpToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl McpToolSpec {
    pub fn new(discovered: &McpDiscoveredTool) -> Arc<Self> {
        let full_name = format!(
            "mcp__{}__{}",
            discovered.client.server_name(),
            discovered.tool.name
        );
        Arc::new(Self {
            id: ToolId(full_name.clone()),
            name: ToolName(full_name),
            description: discovered.tool.description.clone(),
            input_schema: InputSchemaRef {
                schema: discovered.tool.input_schema.clone(),
            },
            output_contract: OutputContract {
                description: "MCP tool output".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: discovered.effect.reads_filesystem,
                writes_filesystem: discovered.effect.writes_filesystem,
                network_access: discovered.effect.network_access,
                side_effects: discovered.effect.side_effects,
            },
        })
    }
}

impl ToolSpecView for McpToolSpec {
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
