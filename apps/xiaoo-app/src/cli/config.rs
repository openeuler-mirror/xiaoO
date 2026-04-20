use agent_types::hook::HookerRegistryConfig;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

/// ~/.config/xiaoo/config.toml
#[derive(Debug, Deserialize, Default)]
pub struct FileConfig {
    pub llm: Option<LlmSection>,
    pub compact: Option<CompactSection>,
    pub skills: Option<SkillsSection>,
    #[serde(default)]
    pub trace: Option<Value>,
    pub hooker: Option<HookerRegistryConfig>,
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
    pub context_window: Option<usize>,
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

impl FileConfig {
    /// Load from the given path, or `~/.config/xiaoo/config.toml` by default.
    pub fn load(path: Option<&str>, debug: bool) -> Self {
        let path = match path {
            Some(p) => PathBuf::from(p),
            None => match dirs::home_dir() {
                Some(home) => home.join(".config").join("xiaoo").join("config.toml"),
                None => return Self::default(),
            },
        };
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

    /// Resolve API key: read the env var named by `api_key_env`.
    pub fn resolve_api_key(&self) -> Option<String> {
        let env_name = self.llm.as_ref()?.api_key_env.as_deref()?;
        std::env::var(env_name).ok().filter(|v| !v.is_empty())
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
