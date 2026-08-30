use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use agent_types::tool::RawToolOutcome;

use super::executor::DeclarativeToolExecutor;
use super::manifest::{
    DeclarativeToolManifest, EffectSection, ExecSection, LoadedDeclarativeTool, OutputSection,
    StdinMode, StdoutMode,
};
use super::spec::DeclarativeToolSpec;

#[derive(Debug, Clone, Deserialize)]
pub struct DeclarativeToolDraft {
    pub name: String,
    pub description: String,
    pub timeout_ms: u64,
    pub input_schema: Value,
    pub output_description: Option<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub stdin: String,
    pub stdout: String,
    #[serde(default)]
    pub env_names: Vec<String>,
    #[serde(default)]
    pub effect: DeclarativeToolDraftEffect,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeclarativeToolDraftEffect {
    #[serde(default)]
    pub reads_filesystem: bool,
    #[serde(default)]
    pub writes_filesystem: bool,
    #[serde(default)]
    pub network_access: bool,
    #[serde(default)]
    pub side_effects: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeclarativeToolRenderReport {
    pub schema_version: u32,
    pub valid: bool,
    pub content: Option<String>,
    pub error: Option<String>,
}

pub fn render_declarative_tool(draft: DeclarativeToolDraft) -> DeclarativeToolRenderReport {
    match render_draft(draft) {
        Ok(content) => DeclarativeToolRenderReport {
            schema_version: 1,
            valid: true,
            content: Some(content),
            error: None,
        },
        Err(error) => DeclarativeToolRenderReport {
            schema_version: 1,
            valid: false,
            content: None,
            error: Some(error),
        },
    }
}

fn render_draft(draft: DeclarativeToolDraft) -> Result<String, String> {
    let stdin = match draft.stdin.as_str() {
        "json" => StdinMode::Json,
        "none" => StdinMode::None,
        value => return Err(format!("unsupported stdin mode `{value}`")),
    };
    let stdout = match draft.stdout.as_str() {
        "text" => StdoutMode::Text,
        "json" => StdoutMode::Json,
        value => return Err(format!("unsupported stdout mode `{value}`")),
    };
    if !draft.input_schema.is_object() {
        return Err("input_schema must be a JSON object".to_string());
    }
    let input_schema = json_to_toml(draft.input_schema)?;
    let manifest = DeclarativeToolManifest {
        name: draft.name,
        description: draft.description,
        timeout_ms: draft.timeout_ms,
        output: draft
            .output_description
            .map(|description| OutputSection { description }),
        effect: EffectSection {
            reads_filesystem: draft.effect.reads_filesystem,
            writes_filesystem: draft.effect.writes_filesystem,
            network_access: draft.effect.network_access,
            side_effects: draft.effect.side_effects,
        },
        input_schema,
        exec: ExecSection {
            command: draft.command,
            args: draft.args,
            stdin,
            stdout,
            env: draft.env_names,
        },
    };
    manifest.validate(Path::new("custom-tool.toml"))?;
    toml::to_string_pretty(&manifest).map_err(|error| format!("failed to render manifest: {error}"))
}

fn json_to_toml(value: Value) -> Result<toml::Value, String> {
    match value {
        Value::Null => Err("input_schema cannot contain null".to_string()),
        Value::Bool(value) => Ok(toml::Value::Boolean(value)),
        Value::Number(value) => value
            .as_i64()
            .map(toml::Value::Integer)
            .or_else(|| value.as_f64().map(toml::Value::Float))
            .ok_or_else(|| "input_schema contains an unsupported number".to_string()),
        Value::String(value) => Ok(toml::Value::String(value)),
        Value::Array(values) => values
            .into_iter()
            .map(json_to_toml)
            .collect::<Result<Vec<_>, _>>()
            .map(toml::Value::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| json_to_toml(value).map(|value| (key, value)))
            .collect::<Result<toml::map::Map<_, _>, _>>()
            .map(toml::Value::Table),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeclarativeToolCatalog {
    pub schema_version: u32,
    pub supported: bool,
    pub directories: Vec<DeclarativeToolDirectory>,
    pub tools: Vec<DeclarativeToolSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeclarativeToolDirectory {
    pub scope: &'static str,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeclarativeToolSummary {
    pub scope: &'static str,
    pub manifest_path: String,
    pub content_hash: String,
    pub status: &'static str,
    pub name: Option<String>,
    pub description: Option<String>,
    pub timeout_ms: Option<u64>,
    pub input_schema: Option<Value>,
    pub output_description: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub stdin: Option<&'static str>,
    pub stdout: Option<&'static str>,
    pub env_names: Vec<String>,
    pub command_available: bool,
    pub effect: Option<DeclarativeToolEffect>,
    pub shadowed_by: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeclarativeToolEffect {
    pub reads_filesystem: bool,
    pub writes_filesystem: bool,
    pub network_access: bool,
    pub side_effects: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeclarativeToolTestReport {
    pub schema_version: u32,
    pub manifest_path: String,
    pub name: Option<String>,
    pub success: bool,
    pub duration_ms: u128,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub async fn test_declarative_tool(
    workspace_root: &Path,
    home_dir: Option<&Path>,
    manifest_path: &Path,
    input: Value,
    supported: bool,
    allow_effects: bool,
) -> DeclarativeToolTestReport {
    let started = Instant::now();
    let path_text = manifest_path.display().to_string();
    let failure = |name: Option<String>, error: String| DeclarativeToolTestReport {
        schema_version: 1,
        manifest_path: path_text.clone(),
        name,
        success: false,
        duration_ms: started.elapsed().as_millis(),
        output: None,
        error: Some(error),
    };
    if !input.is_object() {
        return failure(None, "tool test input must be a JSON object".to_string());
    }
    let canonical = match std::fs::canonicalize(manifest_path) {
        Ok(path) => path,
        Err(error) => return failure(None, format!("failed to resolve manifest: {error}")),
    };
    let allowed_roots = [
        Some(workspace_root.join(".xiaoo/tools")),
        home_dir.map(|home| home.join(".xiaoo/tools")),
    ];
    if !allowed_roots
        .into_iter()
        .flatten()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|allowed| canonical.starts_with(allowed)))
    {
        return failure(
            None,
            "manifest resolves outside the discovered .xiaoo/tools directories".to_string(),
        );
    }
    let catalog = declarative_tool_catalog(Some(workspace_root), home_dir, supported);
    let selected = catalog.tools.iter().find(|tool| {
        std::fs::canonicalize(&tool.manifest_path).is_ok_and(|candidate| candidate == canonical)
    });
    let Some(selected) = selected else {
        return failure(
            None,
            "manifest must be inside a discovered .xiaoo/tools directory".to_string(),
        );
    };
    if selected.status != "active" {
        return failure(
            selected.name.clone(),
            format!(
                "only an active custom tool can be tested (status={})",
                selected.status
            ),
        );
    }
    let loaded = match LoadedDeclarativeTool::load(&canonical) {
        Ok(tool) => tool,
        Err(error) => return failure(selected.name.clone(), error),
    };
    let has_effects = loaded.manifest.effect.reads_filesystem
        || loaded.manifest.effect.writes_filesystem
        || loaded.manifest.effect.network_access
        || loaded.manifest.effect.side_effects;
    if has_effects && !allow_effects {
        return failure(
            Some(loaded.manifest.name),
            "tool declares external effects; explicit effect confirmation is required".to_string(),
        );
    }
    let name = loaded.manifest.name.clone();
    let spec = Arc::new(DeclarativeToolSpec::from_loaded_tool(&loaded));
    let executor = DeclarativeToolExecutor::from_loaded_tool(spec, &loaded);
    match executor.invoke_for_test(input, workspace_root).await {
        Ok(RawToolOutcome::Success { output }) => DeclarativeToolTestReport {
            schema_version: 1,
            manifest_path: path_text,
            name: Some(name),
            success: true,
            duration_ms: started.elapsed().as_millis(),
            output: Some(bounded_output(output)),
            error: None,
        },
        Ok(RawToolOutcome::Error { message }) => failure(Some(name), bounded_output(message)),
        Err(error) => failure(Some(name), bounded_output(error.to_string())),
    }
}

fn bounded_output(value: String) -> String {
    const MAX_CHARS: usize = 65_536;
    let mut chars = value.chars();
    let output = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{output}\n… output truncated by config test")
    } else {
        output
    }
}

pub fn declarative_tool_catalog(
    workspace_root: Option<&Path>,
    home_dir: Option<&Path>,
    supported: bool,
) -> DeclarativeToolCatalog {
    let directories = discovery_directories(workspace_root, home_dir);
    let mut seen = HashMap::<String, String>::new();
    let mut tools = Vec::new();
    for directory in &directories {
        for path in manifest_paths(Path::new(&directory.path)) {
            match LoadedDeclarativeTool::load(&path) {
                Ok(loaded) => {
                    let name = loaded.manifest.name.clone();
                    let shadowed_by = seen.get(&name).cloned();
                    if shadowed_by.is_none() {
                        seen.insert(name, path.display().to_string());
                    }
                    tools.push(valid_summary(
                        directory.scope,
                        loaded,
                        supported,
                        shadowed_by,
                    ));
                }
                Err(error) => tools.push(invalid_summary(directory.scope, path, error)),
            }
        }
    }
    DeclarativeToolCatalog {
        schema_version: 1,
        supported,
        directories,
        tools,
    }
}

fn discovery_directories(
    workspace_root: Option<&Path>,
    home_dir: Option<&Path>,
) -> Vec<DeclarativeToolDirectory> {
    let mut directories = Vec::new();
    if let Some(workspace) = workspace_root {
        let path = workspace.join(".xiaoo").join("tools");
        directories.push(directory("workspace", path));
    }
    if let Some(home) = home_dir {
        let path = home.join(".xiaoo").join("tools");
        if !directories
            .iter()
            .any(|item| item.path == path.display().to_string())
        {
            directories.push(directory("global", path));
        }
    }
    directories
}

fn directory(scope: &'static str, path: PathBuf) -> DeclarativeToolDirectory {
    DeclarativeToolDirectory {
        scope,
        exists: path.is_dir(),
        path: path.display().to_string(),
    }
}

fn manifest_paths(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn valid_summary(
    scope: &'static str,
    loaded: LoadedDeclarativeTool,
    supported: bool,
    shadowed_by: Option<String>,
) -> DeclarativeToolSummary {
    let command_available = find_command(&loaded.manifest.exec.command, &loaded.tool_dir).is_some();
    let status = if shadowed_by.is_some() {
        "shadowed"
    } else if !supported {
        "unsupported_backend"
    } else if !command_available {
        "command_missing"
    } else {
        "active"
    };
    DeclarativeToolSummary {
        scope,
        manifest_path: loaded.manifest_path.display().to_string(),
        content_hash: file_hash(&loaded.manifest_path),
        status,
        name: Some(loaded.manifest.name),
        description: Some(loaded.manifest.description),
        timeout_ms: Some(loaded.manifest.timeout_ms),
        input_schema: Some(loaded.input_schema_json),
        output_description: loaded.manifest.output.map(|output| output.description),
        command: Some(loaded.manifest.exec.command),
        args: loaded.manifest.exec.args,
        stdin: Some(match loaded.manifest.exec.stdin {
            StdinMode::Json => "json",
            StdinMode::None => "none",
        }),
        stdout: Some(match loaded.manifest.exec.stdout {
            StdoutMode::Text => "text",
            StdoutMode::Json => "json",
        }),
        env_names: loaded.manifest.exec.env,
        command_available,
        effect: Some(DeclarativeToolEffect {
            reads_filesystem: loaded.manifest.effect.reads_filesystem,
            writes_filesystem: loaded.manifest.effect.writes_filesystem,
            network_access: loaded.manifest.effect.network_access,
            side_effects: loaded.manifest.effect.side_effects,
        }),
        shadowed_by,
        error: None,
    }
}

fn invalid_summary(scope: &'static str, path: PathBuf, error: String) -> DeclarativeToolSummary {
    DeclarativeToolSummary {
        scope,
        manifest_path: path.display().to_string(),
        content_hash: file_hash(&path),
        status: "invalid",
        name: None,
        description: None,
        timeout_ms: None,
        input_schema: None,
        output_description: None,
        command: None,
        args: Vec::new(),
        stdin: None,
        stdout: None,
        env_names: Vec::new(),
        command_available: false,
        effect: None,
        shadowed_by: None,
        error: Some(error),
    }
}

fn file_hash(path: &Path) -> String {
    let content = std::fs::read(path).unwrap_or_default();
    format!("{:x}", Sha256::digest(content))
}

fn find_command(command: &str, tool_dir: &Path) -> Option<PathBuf> {
    let candidate = if command == "~" || command.starts_with("~/") {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        command
            .strip_prefix("~/")
            .map(|suffix| home.join(suffix))
            .unwrap_or(home)
    } else {
        let path = Path::new(command);
        if path.is_absolute() {
            path.to_path_buf()
        } else if command.starts_with("./") || command.starts_with("../") {
            tool_dir.join(path)
        } else {
            return std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .flat_map(|directory| command_candidates(&directory, command))
                .find(|path| path.is_file());
        }
    };
    candidate.is_file().then_some(candidate)
}

fn command_candidates(directory: &Path, command: &str) -> Vec<PathBuf> {
    let candidate = directory.join(command);
    #[cfg(windows)]
    return vec![candidate, directory.join(format!("{command}.exe"))];
    #[cfg(not(windows))]
    vec![candidate]
}

#[cfg(test)]
mod tests {
    use super::{
        declarative_tool_catalog, render_declarative_tool, test_declarative_tool,
        DeclarativeToolDraft, DeclarativeToolDraftEffect,
    };
    use serde_json::json;
    use std::fs;

    #[test]
    fn reports_invalid_shadowed_and_unsupported_manifests() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let home = temp.path().join("home");
        let workspace_tools = workspace.join(".xiaoo/tools");
        let home_tools = home.join(".xiaoo/tools");
        fs::create_dir_all(&workspace_tools).expect("workspace tools");
        fs::create_dir_all(&home_tools).expect("home tools");
        let manifest = |description: &str| {
            format!(
            "name = \"echo\"\ndescription = \"{description}\"\n[input_schema]\ntype = \"object\"\n[exec]\ncommand = \"missing-catalog-command\"\n"
        )
        };
        fs::write(workspace_tools.join("echo.toml"), manifest("workspace"))
            .expect("workspace manifest");
        fs::write(home_tools.join("echo.toml"), manifest("global")).expect("global manifest");
        fs::write(home_tools.join("invalid.toml"), "name = [").expect("invalid manifest");

        let catalog = declarative_tool_catalog(Some(&workspace), Some(&home), false);
        assert!(!catalog.supported);
        assert_eq!(catalog.tools.len(), 3);
        assert!(catalog
            .tools
            .iter()
            .all(|tool| tool.content_hash.len() == 64));
        assert_eq!(catalog.tools[0].status, "unsupported_backend");
        assert!(catalog.tools.iter().any(|tool| tool.status == "shadowed"));
        assert!(catalog.tools.iter().any(|tool| tool.status == "invalid"));
    }

    #[test]
    fn renders_and_reloads_a_valid_manifest() {
        let report = render_declarative_tool(DeclarativeToolDraft {
            name: "echo_payload".to_string(),
            description: "Echo input".to_string(),
            timeout_ms: 5_000,
            input_schema: json!({"type": "object", "required": ["message"]}),
            output_description: Some("Echo result".to_string()),
            command: "sh".to_string(),
            args: vec!["./echo.sh".to_string()],
            stdin: "json".to_string(),
            stdout: "text".to_string(),
            env_names: vec!["TOKEN".to_string()],
            effect: DeclarativeToolDraftEffect {
                side_effects: true,
                ..DeclarativeToolDraftEffect::default()
            },
        });
        assert!(report.valid);
        let content = report.content.expect("rendered content");
        let parsed: toml::Value = toml::from_str(&content).expect("valid TOML");
        assert_eq!(parsed["name"].as_str(), Some("echo_payload"));
        assert_eq!(parsed["exec"]["stdin"].as_str(), Some("json"));
        assert_eq!(parsed["effect"]["side_effects"].as_bool(), Some(true));
    }

    #[test]
    fn rejects_invalid_visual_drafts() {
        let report = render_declarative_tool(DeclarativeToolDraft {
            name: "bad name".to_string(),
            description: String::new(),
            timeout_ms: 0,
            input_schema: json!([]),
            output_description: None,
            command: String::new(),
            args: Vec::new(),
            stdin: "binary".to_string(),
            stdout: "text".to_string(),
            env_names: Vec::new(),
            effect: DeclarativeToolDraftEffect::default(),
        });
        assert!(!report.valid);
        assert!(report.content.is_none());
        assert!(report.error.is_some());
    }

    #[tokio::test]
    async fn tests_only_active_tools_and_requires_effect_confirmation() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let tools = workspace.join(".xiaoo/tools");
        fs::create_dir_all(&tools).expect("tool dir");
        let path = tools.join("echo.toml");
        fs::write(
            &path,
            "name = \"echo\"\ndescription = \"echo\"\n[input_schema]\ntype = \"object\"\n[exec]\ncommand = \"sh\"\nargs = [\"-c\", \"printf tested\"]\nstdin = \"none\"\n",
        )
        .expect("manifest");
        let report = test_declarative_tool(&workspace, None, &path, json!({}), true, false).await;
        assert!(report.success);
        assert_eq!(report.output.as_deref(), Some("tested"));

        fs::write(
            &path,
            "name = \"echo\"\ndescription = \"echo\"\n[input_schema]\ntype = \"object\"\n[effect]\nside_effects = true\n[exec]\ncommand = \"sh\"\nargs = [\"-c\", \"printf tested\"]\nstdin = \"none\"\n",
        )
        .expect("effect manifest");
        let denied = test_declarative_tool(&workspace, None, &path, json!({}), true, false).await;
        assert!(!denied.success);
        assert!(denied
            .error
            .as_deref()
            .is_some_and(|error| error.contains("confirmation")));
    }
}
