use crate::backend::GatewayBackendConfig;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use xiaoo_api::chat::HookerRegistryConfig;
use xiaoo_api::chat::ReasoningEffort;
use xiaoo_api::llm::ProtocolFamily;
use xiaoo_shared::builtin_agent_roles::{PLAN_AGENT_DESCRIPTION, PLAN_AGENT_ID, PLAN_AGENT_PROMPT};
use xiaoo_shared::gateway::MemoryAutomationConfig;
use xiaoo_shared::lsp_support::{AutoInstall, LspServiceRegistry, ServerConfig};
use xiaoo_shared::mcp_support::{self, McpSection};
use xiaoo_shared::skills_support::SkillsConfig as ResolvedSkillsConfig;

const DEFAULT_AGENT_ID: &str = "main";
const DEFAULT_LLM_MAX_TOKENS: u32 = 16384;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub disabled_servers: Vec<String>,
    #[serde(default)]
    pub extra_servers: Vec<ExtraServerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraServerConfig {
    pub id: String,
    pub extensions: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
    pub language_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub trace: Option<Value>,
    #[serde(default)]
    pub skills: Option<SkillsSection>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentRoleConfig>,
    #[serde(default)]
    pub subagent: BTreeMap<String, SubagentRoleConfig>,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub hooker: HookerRegistryConfig,
    #[serde(default)]
    pub operation_backend: Option<GatewayBackendConfig>,
    #[serde(default)]
    pub lsp: Option<LspConfig>,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub mcp: McpSection,
    #[serde(default)]
    pub memory_automation: MemoryAutomationConfig,
    /// Effective MCP servers after adding optional `.mcp.json` entries. This
    /// is runtime-only so TUI config saves never copy imported servers into
    /// `config.toml`.
    #[serde(skip)]
    runtime_mcp_servers: Option<Vec<mcp_support::McpServerConfig>>,
    /// Preserve daemon- or plugin-owned top-level sections when the TUI
    /// rewrites config.toml after changing providers or other UI settings.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub remote: Option<RemoteConfig>,
    #[serde(default)]
    pub agent_order: Vec<String>,
    /// Redact secrets echoed by the assistant in the TUI display. Default
    /// `false` (local TUI shows the transcript as-is); set `true` for
    /// shoulder-surf protection. Only affects the display sink — raw secrets
    /// remain in history/snapshots regardless.
    #[serde(default = "default_false")]
    pub redact_secrets_display: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemoteConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub bearer_token_env: Option<String>,
    #[serde(default)]
    pub auto_connect: bool,
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
    pub reasoning_effort: ReasoningEffort,
    #[serde(default = "default_false")]
    pub kvcache_enabled: bool,
    #[serde(default = "default_false")]
    pub kvcache_debug_enabled: bool,
}

fn default_false() -> bool {
    false
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            api_key_env: None,
            api_base: String::new(),
            max_tokens: default_llm_max_tokens(),
            reasoning_effort: ReasoningEffort::Off,
            kvcache_enabled: false,
            kvcache_debug_enabled: false,
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
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubagentRoleConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkillsSection {
    #[serde(default)]
    pub dirs: Option<Vec<String>>,
    #[serde(default)]
    pub allow_scripts: Option<bool>,
    #[serde(default)]
    pub disabled: Vec<String>,
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
        let mut persisted = self.clone();
        persisted.agent.remove(PLAN_AGENT_ID);
        fs::write(
            path,
            toml::to_string_pretty(&persisted).context("failed to serialize config")?,
        )
        .with_context(|| format!("failed to write config file {}", path.display()))?;
        Ok(())
    }

    pub fn load_runtime_mcp_servers(
        &mut self,
        explicit_path: Option<&Path>,
        workspace: &Path,
        home: Option<&Path>,
        toml_source: &Path,
    ) -> Result<(), mcp_support::McpConfigError> {
        self.runtime_mcp_servers = Some(load_merged_mcp_servers(
            &self.mcp.servers,
            explicit_path,
            workspace,
            home,
            toml_source,
        )?);
        Ok(())
    }

    pub fn mcp_servers(&self) -> &[mcp_support::McpServerConfig] {
        self.runtime_mcp_servers
            .as_deref()
            .unwrap_or(&self.mcp.servers)
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
        let mut ordered = Vec::new();
        let mut seen = BTreeSet::new();

        for configured_role_id in &self.tui.agent_order {
            let configured_role_id = configured_role_id.trim();
            if configured_role_id.is_empty() {
                continue;
            }

            if let Some(role_id) = self
                .agent
                .keys()
                .find(|role_id| role_id.eq_ignore_ascii_case(configured_role_id))
                .cloned()
            {
                if seen.insert(role_id.clone()) {
                    ordered.push(role_id);
                }
            }
        }

        for role_id in self.agent.keys() {
            if seen.insert(role_id.clone()) {
                ordered.push(role_id.clone());
            }
        }

        ordered
    }

    pub fn agent_role(&self, role_id: &str) -> Option<&AgentRoleConfig> {
        self.agent.get(role_id)
    }

    pub fn resolve_skills_config(&self) -> ResolvedSkillsConfig {
        let mut skills_dirs = Vec::new();

        // Priority 1: Project level (highest)
        skills_dirs.push(PathBuf::from(".xiaoo/skills"));

        // Priority 2: Config file user dirs (medium)
        if let Some(skills) = self.skills.as_ref() {
            if let Some(extra_dirs) = skills.dirs.as_ref() {
                for dir in extra_dirs {
                    let path = PathBuf::from(dir);
                    let dir_str = path.to_string_lossy();
                    if dir_str != ".xiaoo/skills"
                        && !dir_str.ends_with("/.xiaoo/skills")
                        && !dir_str.ends_with("\\.xiaoo\\skills")
                        && dir_str != "/usr/lib/.xiaoo/skills"
                    {
                        skills_dirs.push(path);
                    }
                }
            }
        }

        // Priority 3: User level
        if let Some(home) = dirs::home_dir() {
            skills_dirs.push(home.join(".xiaoo").join("skills"));
        }

        // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
        skills_dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

        ResolvedSkillsConfig {
            skills_dirs,
            allow_scripts: self
                .skills
                .as_ref()
                .and_then(|skills| skills.allow_scripts)
                .unwrap_or(false),
            disabled: self
                .skills
                .as_ref()
                .map(|skills| skills.disabled.iter().cloned().collect())
                .unwrap_or_default(),
            ..ResolvedSkillsConfig::default()
        }
    }

    pub fn build_lsp_registry(&self) -> Option<Arc<LspServiceRegistry>> {
        let lsp = self.lsp.as_ref()?;
        if !lsp.enabled {
            return None;
        }
        let extra = build_extra_server_configs(&lsp.extra_servers);
        Some(Arc::new(LspServiceRegistry::new_with_disabled(
            extra,
            lsp.disabled_servers.clone(),
        )))
    }
}

pub(crate) fn load_merged_mcp_servers(
    toml_servers: &[mcp_support::McpServerConfig],
    explicit_path: Option<&Path>,
    workspace: &Path,
    home: Option<&Path>,
    toml_source: &Path,
) -> Result<Vec<mcp_support::McpServerConfig>, mcp_support::McpConfigError> {
    let json_source = mcp_support::resolve_json_config_path(explicit_path, workspace, home);
    let json_servers = mcp_support::load_json_servers(explicit_path, workspace, home)?;
    let fallback_json_source = workspace.join(".mcp.json");
    mcp_support::merge_server_configs(
        toml_servers.to_vec(),
        json_servers,
        toml_source,
        json_source.as_deref().unwrap_or(&fallback_json_source),
    )
}

pub fn require_tui_bootstrap_config(config: Option<Config>, config_path: &Path) -> Result<Config> {
    let mut config = config
        .ok_or_else(|| anyhow::anyhow!("config file not found: {}", config_path.display()))?;
    install_builtin_agent_roles(&mut config.agent)
        .with_context(|| format!("invalid TUI config {}", config_path.display()))?;

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

    if config.llm.max_tokens == 0 {
        bail!(
            "invalid TUI config {}: [llm].max_tokens must be > 0",
            config_path.display()
        );
    }

    config.validate_default_agent_id().with_context(|| {
        format!(
            "invalid TUI config {}: agents.default_agent_id validation failed",
            config_path.display()
        )
    })?;

    Ok(config)
}

fn install_builtin_agent_roles(agent_roles: &mut BTreeMap<String, AgentRoleConfig>) -> Result<()> {
    if agent_roles.contains_key(PLAN_AGENT_ID) {
        bail!("agent role `{PLAN_AGENT_ID}` is builtin and cannot be overridden in config");
    }

    agent_roles.insert(
        PLAN_AGENT_ID.to_string(),
        AgentRoleConfig {
            description: PLAN_AGENT_DESCRIPTION.to_string(),
            prompt: Some(PLAN_AGENT_PROMPT.to_string()),
            max_turns: None,
            tools: BTreeMap::from([
                ("bash".to_string(), false),
                ("file_edit".to_string(), false),
                ("file_write".to_string(), false),
                ("send_file".to_string(), false),
                ("spawn_subagent".to_string(), false),
            ]),
        },
    );

    Ok(())
}

pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()> {
    xiaoo_shared::llm_secrets::save_llm_secret(config_path, env_name, secret)
}

pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()> {
    xiaoo_shared::llm_secrets::inject_llm_secrets_into_env(config_path)
}

fn build_extra_server_configs(extra_servers: &[ExtraServerConfig]) -> Vec<ServerConfig> {
    extra_servers
        .iter()
        .map(|c| {
            let id: &'static str = Box::leak(c.id.clone().into_boxed_str());
            let command: &'static str = Box::leak(c.command.clone().into_boxed_str());
            let language_id: &'static str = Box::leak(c.language_id.clone().into_boxed_str());
            let extensions: &'static [&'static str] = Box::leak(
                c.extensions
                    .iter()
                    .map(|e| -> &'static str { Box::leak(e.clone().into_boxed_str()) })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            let args: &'static [&'static str] = Box::leak(
                c.args
                    .iter()
                    .map(|a| -> &'static str { Box::leak(a.clone().into_boxed_str()) })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            let root_markers: &'static [&'static str] = Box::leak(
                c.root_markers
                    .iter()
                    .map(|m| -> &'static str { Box::leak(m.clone().into_boxed_str()) })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            ServerConfig {
                id,
                extensions,
                command,
                args,
                root_markers,
                language_id,
                initialization_options: None,
                auto_install: AutoInstall::None,
            }
        })
        .collect()
}

fn default_agent_id() -> String {
    DEFAULT_AGENT_ID.to_string()
}

fn default_llm_max_tokens() -> u32 {
    DEFAULT_LLM_MAX_TOKENS
}

pub fn resolve_context_window(config: &Config) -> Option<usize> {
    if let Some(known) = xiaoo_api::llm::get_known_model_context_length(&config.llm.model) {
        return usize::try_from(known).ok();
    }

    match xiaoo_api::llm::resolve_protocol_family(&config.llm.provider)? {
        ProtocolFamily::OpenAiCompatible | ProtocolFamily::Ollama | ProtocolFamily::Zhipu => {
            Some(128_000)
        }
        ProtocolFamily::Anthropic => Some(200_000),
        ProtocolFamily::Gemini => Some(1_000_000),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/support/config_test.rs"]
mod tests;
