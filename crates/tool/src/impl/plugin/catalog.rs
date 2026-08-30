use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::manifest::{LoadedDeclarativeTool, StdinMode, StdoutMode};

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
    use super::declarative_tool_catalog;
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
        assert_eq!(catalog.tools[0].status, "unsupported_backend");
        assert!(catalog.tools.iter().any(|tool| tool.status == "shadowed"));
        assert!(catalog.tools.iter().any(|tool| tool.status == "invalid"));
    }
}
