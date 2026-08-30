use crate::daemon_config::DaemonConfig;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AgentCatalogReport {
    pub schema_version: u32,
    pub default_agent_id: String,
    pub agents: Vec<AgentSummary>,
}

#[derive(Debug, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub default: bool,
    pub profile_id: Option<String>,
    pub profile_available: bool,
    pub workspace: Option<String>,
    pub workspace_exists: bool,
    pub system_prompt: Option<String>,
    pub uses_default_system_prompt: bool,
    pub explicit: bool,
}

pub fn agent_catalog(config: &DaemonConfig) -> Result<AgentCatalogReport> {
    let resolved = config.resolve_agent()?;
    if config.app.agents.list.is_empty() {
        return Ok(AgentCatalogReport {
            schema_version: 1,
            default_agent_id: resolved.id.clone(),
            agents: vec![AgentSummary {
                id: resolved.id,
                default: true,
                profile_available: resolved
                    .profile_id
                    .as_deref()
                    .is_none_or(|id| config.app.llm.profile(id).is_ok()),
                profile_id: resolved.profile_id,
                workspace_exists: resolved.workspace_root.is_dir(),
                workspace: Some(resolved.workspace_root.display().to_string()),
                system_prompt: None,
                uses_default_system_prompt: true,
                explicit: false,
            }],
        });
    }
    let agents = config
        .app
        .agents
        .list
        .iter()
        .map(|agent| {
            let profile_id = agent
                .profile_id
                .clone()
                .or_else(|| config.app.llm.active_profile.clone());
            AgentSummary {
                id: agent.id.clone(),
                default: agent.id == resolved.id,
                profile_available: profile_id
                    .as_deref()
                    .is_none_or(|id| config.app.llm.profile(id).is_ok()),
                profile_id,
                workspace_exists: agent.workspace.as_ref().is_some_and(|path| path.is_dir()),
                workspace: agent
                    .workspace
                    .as_ref()
                    .map(|path| path.display().to_string()),
                system_prompt: agent.system_prompt.clone(),
                uses_default_system_prompt: agent.system_prompt.is_none(),
                explicit: true,
            }
        })
        .collect();
    Ok(AgentCatalogReport {
        schema_version: 1,
        default_agent_id: resolved.id,
        agents,
    })
}

#[cfg(test)]
mod tests {
    use super::agent_catalog;
    use crate::daemon_config::DaemonConfig;

    #[test]
    fn reports_profile_references_and_default_agent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[llm]
active_profile = "local"
[llm.profiles.local]
provider = "ollama"
model = "qwen"
[agents]
default_agent_id = "review"
[[agents.list]]
id = "review"
profile_id = "local"
system_prompt = "Review carefully"
"#,
        )
        .expect("config");
        let config = DaemonConfig::load_from(&config_path).expect("load config");
        let report = agent_catalog(&config).expect("catalog");
        assert_eq!(report.default_agent_id, "review");
        assert!(report.agents[0].default);
        assert!(report.agents[0].profile_available);
        assert_eq!(
            report.agents[0].system_prompt.as_deref(),
            Some("Review carefully")
        );
    }
}
