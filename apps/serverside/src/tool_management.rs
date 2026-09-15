use crate::daemon_config::DaemonConfig;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use xiaoo_shared::gateway::tool_assembly::{
    discover_tools, SubagentRoleSpec, ToolAssemblyInput, ToolCatalogEntry,
};

#[derive(Debug, Serialize)]
pub struct ToolCatalogReport {
    pub schema_version: u32,
    pub tools: Vec<ToolSummary>,
}

#[derive(Debug, Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    pub source: &'static str,
    pub effect: ToolEffectSummary,
}

#[derive(Debug, Serialize)]
pub struct ToolEffectSummary {
    pub reads_filesystem: bool,
    pub writes_filesystem: bool,
    pub network_access: bool,
    pub side_effects: bool,
}

pub async fn tool_catalog(config: &DaemonConfig, workspace: &Path) -> Result<ToolCatalogReport> {
    let subagent_roles: BTreeMap<String, SubagentRoleSpec> = config
        .app
        .subagent
        .iter()
        .map(|(id, role)| {
            (
                id.clone(),
                SubagentRoleSpec {
                    description: role.description.clone(),
                    prompt: role.prompt.clone(),
                    max_turns: role.max_turns,
                    tools: role.tools.clone(),
                },
            )
        })
        .collect();
    let tools = discover_tools(&ToolAssemblyInput {
        workspace_root: Some(workspace.to_path_buf()),
        disable_plugin_tools: config
            .server_operation_backend()
            .map(|backend| backend.kind == "e2b")
            .unwrap_or(false),
        lsp_registry: config.build_lsp_registry(),
        mcp_servers: Some(config.app.mcp.servers.clone()),
        subagent_roles,
        ..ToolAssemblyInput::default()
    })
    .await?
    .into_iter()
    .map(tool_summary)
    .collect();
    Ok(ToolCatalogReport {
        schema_version: 1,
        tools,
    })
}

fn tool_summary(tool: ToolCatalogEntry) -> ToolSummary {
    ToolSummary {
        name: tool.name,
        description: tool.description,
        source: tool.source.as_str(),
        effect: ToolEffectSummary {
            reads_filesystem: tool.effect.reads_filesystem,
            writes_filesystem: tool.effect.writes_filesystem,
            network_access: tool.effect.network_access,
            side_effects: tool.effect.side_effects,
        },
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/serverside/tool_management_test.rs"]
mod tests;
