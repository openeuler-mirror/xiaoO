use agent_types::hooker::HookerRegistryConfig;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json;
use skill::SkillsConfig;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use xiaoo_app::channels::feishu::FeishuConfig;

const DEFAULT_OUTPUT_TOKENS: usize = 128000;
const DEFAULT_SYSTEM_PROMPT: &str = "You are XiaoO, an enterprise agent operating system assistant. Respond clearly, accurately, and in plain text suitable for enterprise chat channels.";

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub llm: LlmConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentRoleConfig>,
    #[serde(default)]
    pub skills: Option<SkillsSection>,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub trace: Option<TraceConfig>,
    #[serde(default)]
    pub compact: Option<CompactConfig>,
    #[serde(default)]
    pub hooker: HookerRegistryConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub context_window: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub feishu: Option<FeishuChannelConfig>,
    #[serde(default)]
    pub interaction_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeishuChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub app_secret_env: Option<String>,
    #[serde(default)]
    pub verification_token: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillsSection {
    #[serde(default)]
    pub dirs: Option<Vec<String>>,
    #[serde(default)]
    pub allow_scripts: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentsConfig {
    #[serde(default)]
    pub default_agent_id: Option<String>,
    #[serde(default)]
    pub list: Vec<AgentConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentRoleConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PathsConfig {
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TraceConfig {
    /// Storage backend identifier: "moirai-sqlite" (default), "stdout", or "noop".
    #[serde(default)]
    pub storage_backend: Option<String>,
    /// Database path for moirai-sqlite backend. Defaults to trace crate's built-in default.
    #[serde(default)]
    pub db_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CompactConfig {
    #[serde(default)]
    pub warning_ratio: Option<f64>,
    #[serde(default)]
    pub auto_compact_ratio: Option<f64>,
    #[serde(default)]
    pub blocking_ratio: Option<f64>,
    #[serde(default)]
    pub snip_stale_after_ms: Option<u64>,
    #[serde(default)]
    pub snip_preserve_tail: Option<usize>,
    #[serde(default)]
    pub collapse_preserve_tail: Option<usize>,
    #[serde(default)]
    pub summary_max_tokens: Option<usize>,
    #[serde(default)]
    pub summary_preserve_tail: Option<usize>,
    #[serde(default)]
    pub summary_llm_max_tokens: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAgentConfig {
    pub id: String,
    pub model: String,
    pub system_prompt: String,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub app: AppConfig,
    pub config_path: PathBuf,
}

impl DaemonConfig {
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let config_path = path.as_ref().to_path_buf();
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read config {}", config_path.display()))?;
        let app: AppConfig = toml::from_str(&content)
            .with_context(|| format!("failed to parse config {}", config_path.display()))?;
        Ok(Self { app, config_path })
    }

    pub fn resolve_agent(&self) -> Result<ResolvedAgentConfig> {
        let default_agent_id = self
            .app
            .agents
            .default_agent_id
            .clone()
            .or_else(|| {
                self.app
                    .agents
                    .list
                    .iter()
                    .find(|agent| agent.default)
                    .map(|agent| agent.id.clone())
            })
            .or_else(|| self.app.agents.list.first().map(|agent| agent.id.clone()))
            .unwrap_or_else(|| "main".to_string());

        let explicit = self
            .app
            .agents
            .list
            .iter()
            .find(|agent| agent.id == default_agent_id)
            .cloned();

        let model = explicit
            .as_ref()
            .and_then(|agent| agent.model.clone())
            .unwrap_or_else(|| self.app.llm.model.clone());
        let system_prompt = explicit
            .as_ref()
            .and_then(|agent| agent.system_prompt.clone())
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());
        let workspace_root = explicit
            .as_ref()
            .and_then(|agent| agent.workspace.clone())
            .or_else(|| {
                self.app
                    .paths
                    .data_dir
                    .as_ref()
                    .map(|data_dir| data_dir.join("workspace").join(default_agent_id.as_str()))
            })
            .unwrap_or_else(|| default_user_workspace_dir(&default_agent_id));

        Ok(ResolvedAgentConfig {
            id: default_agent_id,
            model,
            system_prompt,
            workspace_root,
        })
    }

    pub fn interaction_timeout_secs(&self) -> u64 {
        self.app.channels.interaction_timeout_secs.unwrap_or(600)
    }

    pub fn feishu_config(&self) -> Result<Option<FeishuConfig>> {
        let Some(feishu) = self.app.channels.feishu.as_ref() else {
            return Ok(None);
        };
        if !feishu.enabled {
            return Ok(None);
        }

        let app_id = required_field("channels.feishu.app_id", feishu.app_id.as_deref())?;
        let app_secret_env = required_field(
            "channels.feishu.app_secret_env",
            feishu.app_secret_env.as_deref(),
        )?;
        let verification_token = required_field(
            "channels.feishu.verification_token",
            feishu.verification_token.as_deref(),
        )?;
        let base_url = feishu
            .base_url
            .clone()
            .unwrap_or_else(|| "https://open.feishu.cn".to_string());

        Ok(Some(FeishuConfig {
            channel_instance_id: None,
            app_id,
            app_secret_env,
            verification_token,
            base_url,
            parse_file_messages: false,
            max_file_download_bytes: 0,
            max_file_text_chars: 0,
        }))
    }

    pub fn resolve_skills_config(&self) -> SkillsConfig {
        let mut skills_dirs = Vec::new();
        if let Some(home) = dirs::home_dir() {
            skills_dirs.push(home.join(".xiaoo").join("skills"));
        }
        if let Some(skills) = self.app.skills.as_ref() {
            if let Some(extra_dirs) = skills.dirs.as_ref() {
                for dir in extra_dirs {
                    skills_dirs.push(PathBuf::from(dir));
                }
            }
        }
        skills_dirs.sort();
        skills_dirs.dedup();

        SkillsConfig {
            skills_dirs,
            allow_scripts: self
                .app
                .skills
                .as_ref()
                .and_then(|skills| skills.allow_scripts)
                .unwrap_or(false),
            ..SkillsConfig::default()
        }
    }

    pub fn max_output_tokens(&self) -> usize {
        self.app.llm.max_tokens.unwrap_or(DEFAULT_OUTPUT_TOKENS)
    }

    pub fn resolve_trace_config(&self) -> serde_json::Value {
        let Some(trace) = self.app.trace.as_ref() else {
            return serde_json::Value::Object(serde_json::Map::new());
        };
        let mut map = serde_json::Map::new();
        if let Some(backend) = &trace.storage_backend {
            map.insert(
                "storage_backend".to_string(),
                serde_json::Value::String(backend.clone()),
            );
        }
        if let Some(db_path) = &trace.db_path {
            map.insert(
                "db_path".to_string(),
                serde_json::Value::String(db_path.clone()),
            );
        }
        serde_json::Value::Object(map)
    }

    pub fn resolve_compact_config(&self) -> Option<&CompactConfig> {
        self.app.compact.as_ref()
    }
}

pub fn resolve_config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    if let Ok(path) = env::var("XIAOO_CONFIG") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    if let Some(home) = home_dir() {
        let xdg = home.join(".config/xiaoo/config.toml");
        if xdg.exists() {
            return Ok(xdg);
        }
    }

    Ok(PathBuf::from("config.toml"))
}

fn required_field(field_name: &str, value: Option<&str>) -> Result<String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        bail!("{field_name} is required when the channel is enabled");
    };
    Ok(value.to_string())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn default_user_workspace_dir(agent_id: &str) -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".xiaoo")
        .join("workspace")
        .join(agent_id)
}

#[cfg(test)]
mod tests {
    use super::{resolve_config_path, AppConfig, DaemonConfig};
    use tempfile::TempDir;

    #[test]
    fn parses_feishu_channel_config() {
        let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [channels.feishu]
            enabled = true
            app_id = "cli_123"
            app_secret_env = "FEISHU_APP_SECRET"
            verification_token = "verify-token"
        "#;

        let config: AppConfig = toml::from_str(content).expect("config should parse");
        let daemon = DaemonConfig {
            app: config,
            config_path: "config.toml".into(),
        };
        let feishu = daemon
            .feishu_config()
            .expect("feishu config should validate")
            .expect("feishu should be enabled");
        assert_eq!(feishu.app_id, "cli_123");
        assert_eq!(feishu.base_url, "https://open.feishu.cn");
    }

    #[test]
    fn resolves_xdg_config_when_present() {
        let temp = TempDir::new().expect("tempdir");
        let xdg_dir = temp.path().join(".config/xiaoo");
        std::fs::create_dir_all(&xdg_dir).expect("create xdg dir");
        let config_path = xdg_dir.join("config.toml");
        std::fs::write(&config_path, "").expect("write config");

        let previous_home = std::env::var_os("HOME");
        std::env::set_var("HOME", temp.path());
        let resolved = resolve_config_path(None).expect("resolve path");
        if let Some(home) = previous_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        assert_eq!(resolved, config_path);
    }

    #[test]
    fn parses_agent_role_presets() {
        let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [agent.code-reviewer]
            description = "Reviews code for best practices and potential issues"
            prompt = "You are a code reviewer."

            [agent.code-reviewer.tools]
            file_write = false
            file_edit = false
        "#;

        let config: AppConfig = toml::from_str(content).expect("config should parse");
        let role = config
            .agent
            .get("code-reviewer")
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
