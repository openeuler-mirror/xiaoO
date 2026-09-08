use crate::daemon_config::{AgentRoleConfig, DaemonConfig, SubagentRoleConfig};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use xiaoo_shared::builtin_agent_roles::PLAN_AGENT_ID;

#[derive(Debug, Serialize)]
pub struct RoleCatalogReport {
    pub schema_version: u32,
    pub agent_roles: Vec<RoleSummary>,
    pub subagent_roles: Vec<RoleSummary>,
}

#[derive(Debug, Serialize)]
pub struct RoleSummary {
    pub id: String,
    pub builtin: bool,
    pub description: String,
    pub prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub tools: BTreeMap<String, bool>,
}

pub fn role_catalog(config_path: &Path) -> Result<RoleCatalogReport> {
    let config = DaemonConfig::load_from(config_path)?;
    Ok(RoleCatalogReport {
        schema_version: 1,
        agent_roles: config
            .app
            .agent
            .into_iter()
            .map(|(id, role)| agent_role_summary(id, role))
            .collect(),
        subagent_roles: config
            .app
            .subagent
            .into_iter()
            .map(|(id, role)| subagent_role_summary(id, role))
            .collect(),
    })
}

fn agent_role_summary(id: String, role: AgentRoleConfig) -> RoleSummary {
    RoleSummary {
        builtin: id == PLAN_AGENT_ID,
        id,
        description: role.description,
        prompt: role.prompt,
        max_turns: role.max_turns,
        tools: role.tools,
    }
}

fn subagent_role_summary(id: String, role: SubagentRoleConfig) -> RoleSummary {
    RoleSummary {
        id,
        builtin: false,
        description: role.description,
        prompt: role.prompt,
        max_turns: role.max_turns,
        tools: role.tools,
    }
}

#[cfg(test)]
mod tests {
    use super::role_catalog;
    use tempfile::TempDir;

    #[test]
    fn lists_effective_agent_and_subagent_roles() {
        let temp = TempDir::new().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[llm]
active_profile = "local"

[llm.profiles.local]
provider = "ollama"
model = "qwen3"

[agent.reviewer]
description = "Review code"
prompt = "Review carefully"
max_turns = 6

[agent.reviewer.tools]
file_write = false

[subagent.researcher]
description = "Research topics"

[subagent.researcher.tools]
web_search = true
"#,
        )
        .expect("write config");

        let report = role_catalog(&config_path).expect("role catalog");
        let reviewer = report
            .agent_roles
            .iter()
            .find(|role| role.id == "reviewer")
            .expect("reviewer role");
        assert!(!reviewer.builtin);
        assert_eq!(reviewer.max_turns, Some(6));
        assert_eq!(reviewer.tools.get("file_write"), Some(&false));
        assert!(report
            .agent_roles
            .iter()
            .any(|role| role.id == "plan" && role.builtin));
        assert!(report
            .subagent_roles
            .iter()
            .any(|role| role.id == "researcher" && !role.builtin));
    }
}
