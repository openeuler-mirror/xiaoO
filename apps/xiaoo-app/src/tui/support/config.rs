use agent_types::hooker::HookerRegistryConfig;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_AGENT_ID: &str = "main";
const LLM_SECRETS_FILE: &str = "llm_secrets.json";
const DEFAULT_LLM_MAX_TOKENS: u32 = 128000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub trace: Option<Value>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentRoleConfig>,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub hooker: HookerRegistryConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub api_base: String,
    #[serde(default = "default_llm_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub context_window: Option<u32>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            api_key_env: None,
            api_base: String::new(),
            max_tokens: default_llm_max_tokens(),
            context_window: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(default = "default_agent_id")]
    pub default_agent_id: String,
    #[serde(default)]
    pub list: Vec<AgentConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    #[serde(default)]
    pub workspace_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            default_agent_id: default_agent_id(),
            list: Vec::new(),
        }
    }
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config file {}", path.display()))
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        fs::write(
            path,
            toml::to_string_pretty(self).context("failed to serialize config")?,
        )
        .with_context(|| format!("failed to write config file {}", path.display()))?;
        Ok(())
    }

    pub fn list_agent_ids(&self) -> Vec<String> {
        self.agents
            .list
            .iter()
            .map(|agent| agent.id.to_lowercase())
            .collect()
    }

    pub fn resolve_default_agent_id(&self) -> String {
        if !self.agents.default_agent_id.trim().is_empty() {
            return self.agents.default_agent_id.to_lowercase();
        }
        self.agents
            .list
            .first()
            .map(|agent| agent.id.to_lowercase())
            .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string())
    }

    pub fn validate_default_agent_id(&self) -> Result<()> {
        let ids = self.list_agent_ids();
        if ids.is_empty() {
            return Ok(());
        }

        let default_agent_id = self.resolve_default_agent_id();
        if ids.contains(&default_agent_id) {
            return Ok(());
        }

        bail!(
            "default agent id {:?} not in agents.list (available: {:?})",
            default_agent_id,
            ids
        )
    }

    pub fn agent_role_ids(&self) -> Vec<String> {
        self.agent.keys().cloned().collect()
    }

    pub fn agent_role(&self, role_id: &str) -> Option<&AgentRoleConfig> {
        self.agent.get(role_id)
    }
}

pub fn require_tui_bootstrap_config(config: Option<Config>, config_path: &Path) -> Result<Config> {
    let config = config
        .ok_or_else(|| anyhow::anyhow!("config file not found: {}", config_path.display()))?;

    if config.llm.provider.trim().is_empty() {
        bail!(
            "invalid TUI config {}: missing [llm].provider",
            config_path.display()
        );
    }
    if config.llm.model.trim().is_empty() {
        bail!(
            "invalid TUI config {}: missing [llm].model",
            config_path.display()
        );
    }

    let context_window = config
        .llm
        .context_window
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "invalid TUI config {}: [llm].context_window must be > 0",
                config_path.display()
            )
        })?;
    let _ = usize::try_from(context_window).map_err(|_| {
        anyhow::anyhow!(
            "invalid TUI config {}: [llm].context_window does not fit platform usize",
            config_path.display()
        )
    })?;

    if config.llm.max_tokens == 0 {
        bail!(
            "invalid TUI config {}: [llm].max_tokens must be > 0",
            config_path.display()
        );
    }

    if let Some(env_name) = config.llm.api_key_env.as_deref() {
        let trimmed = env_name.trim();
        if trimmed.is_empty() {
            bail!(
                "invalid TUI config {}: [llm].api_key_env must not be empty when set",
                config_path.display()
            );
        }
        let env_value = std::env::var(trimmed).unwrap_or_default();
        if env_value.trim().is_empty() {
            bail!(
                "invalid TUI config {}: env var {} is not set",
                config_path.display(),
                trimmed
            );
        }
    }

    config.validate_default_agent_id().with_context(|| {
        format!(
            "invalid TUI config {}: agents.default_agent_id validation failed",
            config_path.display()
        )
    })?;

    Ok(config)
}

pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()> {
    let secrets_path = llm_secrets_path(config_path);
    if let Some(parent) = secrets_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create secrets directory {}", parent.display()))?;
    }

    let mut secrets = load_llm_secrets(&secrets_path)?;
    secrets.insert(env_name.to_string(), secret.to_string());
    fs::write(
        &secrets_path,
        serde_json::to_vec_pretty(&secrets).context("failed to serialize llm secrets")?,
    )
    .with_context(|| format!("failed to write secrets file {}", secrets_path.display()))?;
    Ok(())
}

pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()> {
    let secrets_path = llm_secrets_path(config_path);
    let secrets = load_llm_secrets(&secrets_path)?;
    for (key, value) in secrets {
        std::env::set_var(key, value);
    }
    Ok(())
}

fn load_llm_secrets(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read secrets file {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse secrets file {}", path.display()))
}

fn llm_secrets_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LLM_SECRETS_FILE)
}

fn default_agent_id() -> String {
    DEFAULT_AGENT_ID.to_string()
}

fn default_llm_max_tokens() -> u32 {
    DEFAULT_LLM_MAX_TOKENS
}

#[cfg(test)]
mod tests {
    use super::{require_tui_bootstrap_config, Config};
    use std::path::Path;

    fn valid_config() -> Config {
        let mut config = Config::default();
        config.llm.provider = "openai".to_string();
        config.llm.model = "gpt-4o".to_string();
        config.llm.max_tokens = 128000;
        config.llm.context_window = Some(128_000);
        config
    }

    #[test]
    fn tui_bootstrap_requires_config_file() {
        let error = require_tui_bootstrap_config(None, Path::new("/tmp/missing.toml"))
            .expect_err("missing config should fail");
        assert!(
            error.to_string().contains("config file not found"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn tui_bootstrap_requires_context_window() {
        let mut config = valid_config();
        config.llm.context_window = None;

        let error = require_tui_bootstrap_config(Some(config), Path::new("/tmp/config.toml"))
            .expect_err("missing context_window should fail");
        assert!(
            error
                .to_string()
                .contains("[llm].context_window must be > 0"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn tui_bootstrap_requires_default_agent_id_in_agents_list() {
        let mut config = valid_config();
        config.agents.default_agent_id = "missing".to_string();
        config.agents.list.push(super::AgentConfig {
            id: "main".to_string(),
            workspace_dir: None,
        });

        let error = require_tui_bootstrap_config(Some(config), Path::new("/tmp/config.toml"))
            .expect_err("default agent id mismatch should fail");
        assert!(
            error
                .to_string()
                .contains("agents.default_agent_id validation failed"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parses_agent_role_presets() {
        let config: Config = toml::from_str(
            r#"
[llm]
provider = "openai"
model = "gpt-4o"
max_tokens = 128000
context_window = 128000

[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer."

[agent.code-reviewer.tools]
file_write = false
file_edit = false
"#,
        )
        .expect("agent role config should parse");

        let role = config
            .agent_role("code-reviewer")
            .expect("code-reviewer role should exist");
        assert_eq!(
            role.description,
            "Reviews code for best practices and potential issues"
        );
        assert_eq!(role.prompt.as_deref(), Some("You are a code reviewer."));
        assert_eq!(role.tools.get("file_write"), Some(&false));
        assert_eq!(role.tools.get("file_edit"), Some(&false));
    }
}
