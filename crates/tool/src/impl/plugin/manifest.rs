use agent_types::tool::spec_types::EffectProfile;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct DeclarativeToolManifest {
    pub name: String,
    pub description: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub output: Option<OutputSection>,
    #[serde(default)]
    pub effect: EffectSection,
    pub input_schema: toml::Value,
    pub exec: ExecSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutputSection {
    #[serde(default = "default_output_description")]
    pub description: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EffectSection {
    #[serde(default)]
    pub reads_filesystem: bool,
    #[serde(default)]
    pub writes_filesystem: bool,
    #[serde(default)]
    pub network_access: bool,
    #[serde(default)]
    pub side_effects: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecSection {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_stdin")]
    pub stdin: StdinMode,
    #[serde(default = "default_stdout")]
    pub stdout: StdoutMode,
    #[serde(default)]
    pub env: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StdinMode {
    Json,
    None,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StdoutMode {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct LoadedDeclarativeTool {
    pub manifest_path: PathBuf,
    pub tool_dir: PathBuf,
    pub manifest: DeclarativeToolManifest,
    pub input_schema_json: Value,
}

impl LoadedDeclarativeTool {
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let manifest: DeclarativeToolManifest = toml::from_str(&content)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        manifest.validate(path)?;
        let input_schema_json = toml_to_json(manifest.input_schema.clone());
        let tool_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        Ok(Self {
            manifest_path: path.to_path_buf(),
            tool_dir,
            manifest,
            input_schema_json,
        })
    }
}

impl DeclarativeToolManifest {
    fn validate(&self, path: &Path) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err(format!("{} has an empty tool name", path.display()));
        }
        if !is_valid_tool_name(&self.name) {
            return Err(format!(
                "{} has invalid tool name '{}'; use letters, numbers, '_' or '-'",
                path.display(),
                self.name
            ));
        }
        if self.description.trim().is_empty() {
            return Err(format!("{} has an empty description", path.display()));
        }
        if self.exec.command.trim().is_empty() {
            return Err(format!("{} has an empty exec.command", path.display()));
        }
        if self.timeout_ms == 0 {
            return Err(format!("{} has timeout_ms=0", path.display()));
        }
        for env_name in &self.exec.env {
            if env_name.trim().is_empty() || env_name.contains('=') {
                return Err(format!(
                    "{} has invalid exec.env entry '{}'; use environment variable names only",
                    path.display(),
                    env_name
                ));
            }
        }
        Ok(())
    }
}

impl From<&EffectSection> for EffectProfile {
    fn from(effect: &EffectSection) -> Self {
        Self {
            reads_filesystem: effect.reads_filesystem,
            writes_filesystem: effect.writes_filesystem,
            network_access: effect.network_access,
            side_effects: effect.side_effects,
        }
    }
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(value) => Value::String(value),
        toml::Value::Integer(value) => Value::Number(value.into()),
        toml::Value::Float(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(value) => Value::Bool(value),
        toml::Value::Datetime(value) => Value::String(value.to_string()),
        toml::Value::Array(values) => Value::Array(values.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, toml_to_json(value)))
                .collect(),
        ),
    }
}

fn is_valid_tool_name(name: &str) -> bool {
    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn default_timeout_ms() -> u64 {
    30_000
}

fn default_output_description() -> String {
    "Tool output".to_string()
}

fn default_stdin() -> StdinMode {
    StdinMode::Json
}

fn default_stdout() -> StdoutMode {
    StdoutMode::Text
}
