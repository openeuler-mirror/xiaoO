use crate::backend::GatewayBackendConfig;
use mcp::McpSection;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use xiaoo_api::chat::HookerRegistryConfig;
use xiaoo_shared::gateway::MemoryAutomationConfig;

const CONFIG_ENV_VAR: &str = "XIAOO_CONFIG";

/// ~/.config/xiaoo/config.toml
#[derive(Debug, Deserialize, Default)]
pub struct FileConfig {
    pub llm: Option<LlmSection>,
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
pub struct SkillsSection {
    /// Additional skill directories to scan (besides the default ~/.xiaoo/skills/).
    pub dirs: Option<Vec<String>>,
    /// Allow skills to include script files (.sh, .bash, etc.).
    pub allow_scripts: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct LlmSection {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub api_base: Option<String>,
    pub kvcache_enabled: Option<bool>,
    pub kvcache_debug_enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct CompactSection {
    pub warning_ratio: Option<f64>,
    pub auto_compact_ratio: Option<f64>,
    pub blocking_ratio: Option<f64>,
    pub snip_stale_after_ms: Option<u64>,
    pub snip_preserve_tail: Option<usize>,
    pub collapse_preserve_tail: Option<usize>,
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
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<toml::Value>(&content) {
                Ok(root) => {
                    if debug {
                        eprintln!("[config] loaded {}", path.display());
                    }
                    Self {
                        llm: parse_optional_section(&root, "llm", &path, debug),
                        compact: parse_optional_section(&root, "compact", &path, debug),
                        trace: parse_optional_section(&root, "trace", &path, debug),
                        skills: parse_optional_section(&root, "skills", &path, debug),
                        hooker: parse_optional_section(&root, "hooker", &path, debug),
                        operation_backend: parse_optional_section(
                            &root,
                            "operation_backend",
                            &path,
                            debug,
                        ),
                        subagent: parse_optional_section(&root, "subagent", &path, debug)
                            .unwrap_or_default(),
                        mcp: parse_optional_section(&root, "mcp", &path, debug).unwrap_or_default(),
                        memory_automation: parse_optional_section(
                            &root,
                            "memory_automation",
                            &path,
                            debug,
                        )
                        .unwrap_or_default(),
                    }
                }
                Err(e) => {
                    eprintln!("[config] parse error in {}: {}", path.display(), e);
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn resolve_mcp_servers(
        &self,
        explicit_path: Option<&Path>,
        workspace: &Path,
        home: Option<&Path>,
        toml_source: &Path,
    ) -> Result<Vec<mcp::McpServerConfig>, mcp::McpConfigError> {
        crate::support::config::load_merged_mcp_servers(
            &self.mcp.servers,
            explicit_path,
            workspace,
            home,
            toml_source,
        )
    }
}

fn parse_optional_section<T>(
    root: &toml::Value,
    key: &str,
    path: &std::path::Path,
    debug: bool,
) -> Option<T>
where
    T: DeserializeOwned,
{
    let section = root.get(key)?.clone();

    match section.try_into() {
        Ok(value) => Some(value),
        Err(error) => {
            if debug {
                eprintln!(
                    "[config] parse error in {} [{}]: {}",
                    path.display(),
                    key,
                    error
                );
            } else {
                eprintln!("[config] parse error in {} [{}]", path.display(), key);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_subagent_config() {
        let config_content = r#"
[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.test_writer]
description = "Test writing specialist"
prompt = "You are a test writing specialist."
max_turns = 8
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = FileConfig::load_from_path(temp_file.path(), false);

        assert_eq!(config.subagent.len(), 2);
        assert!(config.subagent.contains_key("code_reviewer"));
        assert!(config.subagent.contains_key("test_writer"));

        let reviewer = config.subagent.get("code_reviewer").unwrap();
        assert_eq!(reviewer.description, "Code review specialist");
        assert_eq!(
            reviewer.prompt,
            Some("You are a code review specialist.".to_string())
        );
        assert_eq!(reviewer.max_turns, Some(5));

        let writer = config.subagent.get("test_writer").unwrap();
        assert_eq!(writer.description, "Test writing specialist");
        assert_eq!(writer.max_turns, Some(8));
    }

    #[test]
    fn test_subagent_tools_config() {
        let config_content = r#"
[llm]
provider = "openai"

[subagent.limited_agent]
description = "Agent with limited tools"
tools = { "bash" = true, "read" = true }
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = FileConfig::load_from_path(temp_file.path(), false);

        assert_eq!(config.subagent.len(), 1);
        let agent = config.subagent.get("limited_agent").unwrap();
        assert_eq!(agent.tools.len(), 2);
        assert_eq!(agent.tools.get("bash"), Some(&true));
        assert_eq!(agent.tools.get("read"), Some(&true));
        assert_eq!(agent.tools.get("write"), None);
    }

    #[test]
    fn test_empty_subagent_config() {
        let config_content = r#"
[llm]
provider = "anthropic"
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = FileConfig::load_from_path(temp_file.path(), false);
        assert_eq!(config.subagent.len(), 0);
    }

    #[test]
    fn test_loads_memory_automation_config() {
        let config_content = r#"
[memory_automation]
enabled = true
server = "ram-a"
recall_top_k = 3
recall_token_budget = 128
context_messages = 2
queue_path = "/tmp/xiaoo-memory-queue.jsonl"
queue_capacity = 32
max_retries = 4
retry_backoff_ms = 50
allowed_agent_roles = ["main", "researcher"]
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = FileConfig::load_from_path(temp_file.path(), false);

        assert!(config.memory_automation.enabled);
        assert_eq!(config.memory_automation.server, "ram-a");
        assert_eq!(config.memory_automation.recall_top_k, 3);
        assert_eq!(config.memory_automation.recall_token_budget, 128);
        assert_eq!(config.memory_automation.context_messages, 2);
        assert_eq!(config.memory_automation.queue_capacity, 32);
        assert_eq!(config.memory_automation.max_retries, 4);
        assert_eq!(config.memory_automation.retry_backoff_ms, 50);
        assert_eq!(
            config.memory_automation.allowed_agent_roles,
            vec!["main".to_string(), "researcher".to_string()]
        );
    }

    #[test]
    fn test_load_linux_dynsandbox_operation_backend_config() {
        let config_content = r#"
[llm]
provider = "openai"

[operation_backend]
kind = "local"

[operation_backend.options.isolation]
kind = "linux_dynsandbox"
allow_network = false
readable_roots = ["/home/alice/project"]
writable_roots = ["/home/alice/project/.xiaoo-tmp"]
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = FileConfig::load_from_path(temp_file.path(), false);

        let backend = config.operation_backend.expect("operation_backend");
        assert_eq!(backend.kind, "local");
        assert_eq!(backend.options["isolation"]["kind"], "linux_dynsandbox");
        assert_eq!(backend.options["isolation"]["allow_network"], false);
        assert_eq!(
            backend.options["isolation"]["readable_roots"][0],
            "/home/alice/project"
        );
        assert_eq!(
            backend.options["isolation"]["writable_roots"][0],
            "/home/alice/project/.xiaoo-tmp"
        );
    }
}
