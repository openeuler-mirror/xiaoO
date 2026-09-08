use crate::daemon_config::DaemonConfig;
use crate::daemon_runtime::ConfiguredRuntimeResolver;
use anyhow::Result;
use serde::Serialize;
use std::time::Instant;

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

#[derive(Debug, Serialize)]
pub struct AgentStartupReport {
    pub schema_version: u32,
    pub agent_id: String,
    pub profile_id: Option<String>,
    pub workspace: Option<String>,
    pub success: bool,
    pub duration_ms: u128,
    pub error: Option<String>,
}

pub async fn test_agent_startup(config: &DaemonConfig, agent_id: &str) -> AgentStartupReport {
    let started = Instant::now();
    let resolved = config.resolve_agent_by_id(agent_id);
    let (profile_id, workspace) = match &resolved {
        Ok(agent) => (
            agent.profile_id.clone(),
            Some(agent.workspace_root.display().to_string()),
        ),
        Err(_) => (None, None),
    };
    let result = match resolved {
        Ok(_) => ConfiguredRuntimeResolver::from_config_for_agent(config, agent_id)
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };
    AgentStartupReport {
        schema_version: 1,
        agent_id: agent_id.to_string(),
        profile_id,
        workspace,
        success: result.is_ok(),
        duration_ms: started.elapsed().as_millis(),
        error: result.err().map(|error| error.to_string()),
    }
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
    use super::{agent_catalog, test_agent_startup};
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

    #[tokio::test]
    async fn tests_selected_agent_runtime_initialization() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "[llm]\nactive_profile = \"local\"\n[llm.profiles.local]\nprovider = \"ollama\"\nmodel = \"qwen\"\napi_base = \"http://127.0.0.1:11434\"\n[[agents.list]]\nid = \"main\"\nprofile_id = \"local\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).expect("path")
            ),
        )
        .expect("config");
        let config = DaemonConfig::load_from(&config_path).expect("load config");
        let report = test_agent_startup(&config, "main").await;
        assert!(report.success, "{:?}", report.error);
        assert_eq!(report.profile_id.as_deref(), Some("local"));
        assert!(workspace.is_dir());

        let missing = test_agent_startup(&config, "missing").await;
        assert!(!missing.success);
        assert!(missing
            .error
            .as_deref()
            .is_some_and(|error| error.contains("does not exist")));
    }
}
