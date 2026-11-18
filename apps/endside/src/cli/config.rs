use crate::backend::GatewayBackendConfig;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use xiaoo_api::chat::HookerRegistryConfig;
use xiaoo_api::llm::{RateLimitSection, RequestHeadersConfig};
use xiaoo_shared::gateway::MemoryAutomationConfig;
use xiaoo_shared::mcp_support::{self, McpSection};
pub use xiaoo_shared::skills_support::SkillsSection;

const CONFIG_ENV_VAR: &str = "XIAOO_CONFIG";

/// ~/.config/xiaoo/config.toml
#[derive(Debug, Deserialize, Default)]
pub struct FileConfig {
    pub llm: Option<LlmSection>,
    #[serde(skip)]
    pub request_headers_error: Option<String>,
    pub compact: Option<CompactSection>,
    pub skills: Option<SkillsSection>,
    #[serde(default)]
    pub trace: Option<Value>,
    pub hooker: Option<HookerRegistryConfig>,
    #[serde(default)]
    pub operation_backend: Option<GatewayBackendConfig>,
    #[serde(default)]
    pub subagent: BTreeMap<String, SubagentRoleConfig>,
    #[serde(default)]
    pub mcp: McpSection,
    #[serde(default)]
    pub memory_automation: MemoryAutomationConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct LlmSection {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub api_base: Option<String>,
    pub kvcache_enabled: Option<bool>,
    pub kvcache_debug_enabled: Option<bool>,
    pub rate_limit: Option<RateLimitSection>,
    #[serde(flatten)]
    pub request_headers: RequestHeadersConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct CompactSection {
    pub auto_compact_ratio: Option<f64>,
    pub blocking_ratio: Option<f64>,
    pub summary_max_tokens: Option<usize>,
    pub summary_preserve_tail: Option<usize>,
    pub summary_llm_max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
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

impl FileConfig {
    pub fn resolve_path(path: Option<&str>) -> Option<PathBuf> {
        if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
            return Some(PathBuf::from(path));
        }

        if let Some(path) = std::env::var_os(CONFIG_ENV_VAR)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
        {
            return Some(path);
        }

        dirs::home_dir().map(|home| home.join(".config").join("xiaoo").join("config.toml"))
    }

    /// Load from the given path, or `~/.config/xiaoo/config.toml` by default.
    pub fn load(path: Option<&str>, debug: bool) -> Self {
        let Some(path) = Self::resolve_path(path) else {
            return Self::default();
        };
        Self::load_from_path(&path, debug)
    }

    pub fn load_from_path(path: &Path, debug: bool) -> Self {
        Self::try_load_from_path(path, debug).unwrap_or_else(|error| {
            eprintln!("[config] {error}");
            Self::default()
        })
    }

    pub fn try_load_from_path(path: &Path, debug: bool) -> Result<Self, String> {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let root = toml::from_str::<toml::Value>(&content)
            .map_err(|_| format!("invalid TOML in {}", path.display()))?;
        let llm = parse_optional_section(&root, "llm", path);
        // A malformed model header table must fail a model run without
        // discarding the unrelated sections used by other CLI commands.
        let request_headers_error = if llm.is_none() {
            root.get("llm")
                .filter(|section| section.is_table())
                .cloned()
                .and_then(|section| section.try_into::<RequestHeadersConfig>().err())
                .map(|_| {
                    format!(
                        "invalid [llm.request_header] or [llm.request_header_env] table in {}",
                        path.display()
                    )
                })
        } else {
            None
        };
        if debug {
            eprintln!("[config] loaded {}", path.display());
        }
        Ok(Self {
            llm,
            request_headers_error,
            compact: parse_optional_section(&root, "compact", path),
            trace: parse_optional_section(&root, "trace", path),
            skills: parse_optional_section(&root, "skills", path),
            hooker: parse_optional_section(&root, "hooker", path),
            operation_backend: parse_optional_section(&root, "operation_backend", path),
            subagent: parse_optional_section(&root, "subagent", path).unwrap_or_default(),
            mcp: parse_optional_section(&root, "mcp", path).unwrap_or_default(),
            memory_automation: parse_optional_section(&root, "memory_automation", path)
                .unwrap_or_default(),
        })
    }

    pub fn resolve_mcp_servers(
        &self,
        explicit_path: Option<&Path>,
        workspace: &Path,
        home: Option<&Path>,
        toml_source: &Path,
    ) -> Result<Vec<mcp_support::McpServerConfig>, mcp_support::McpConfigError> {
        crate::support::config::load_merged_mcp_servers(
            &self.mcp.servers,
            explicit_path,
            workspace,
            home,
            toml_source,
        )
    }

    pub fn resolve_api_key(&self) -> Option<String> {
        self.llm
            .as_ref()
            .and_then(|l| l.api_key_env.as_ref())
            .and_then(|env_name| std::env::var(env_name).ok())
            .filter(|key| !key.trim().is_empty())
    }
}

fn parse_optional_section<T>(root: &toml::Value, key: &str, path: &std::path::Path) -> Option<T>
where
    T: DeserializeOwned,
{
    let section = root.get(key)?.clone();

    match section.try_into() {
        Ok(value) => Some(value),
        Err(_) => {
            eprintln!("[config] parse error in {} [{}]", path.display(), key);
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/cli/config_test.rs"]
mod tests;
