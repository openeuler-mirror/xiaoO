//! Plugin tool sources.

use agent_contracts::tool::{DiscoveredTool, ToolSource};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::executor::DeclarativeToolExecutor;
use super::manifest::LoadedDeclarativeTool;
use super::spec::DeclarativeToolSpec;

/// A plugin tool source.
pub struct PluginToolSource {
    workspace_root: Option<PathBuf>,
    home_dir: Option<PathBuf>,
}

impl PluginToolSource {
    /// Creates a new plugin tool source.
    pub fn new(workspace_root: Option<PathBuf>) -> Self {
        Self::with_home(workspace_root, std::env::var_os("HOME").map(PathBuf::from))
    }

    /// Creates a plugin tool source with an explicit home directory.
    pub fn with_home(workspace_root: Option<PathBuf>, home_dir: Option<PathBuf>) -> Self {
        Self {
            workspace_root,
            home_dir,
        }
    }

    fn discovery_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        let workspace_root = self
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok());
        if let Some(workspace_root) = workspace_root {
            dirs.push(workspace_root.join(".xiaoo").join("tools"));
        }

        if let Some(home) = &self.home_dir {
            let home_tools = home.join(".xiaoo").join("tools");
            if !dirs.contains(&home_tools) {
                dirs.push(home_tools);
            }
        }

        dirs
    }

    fn discover_dir(dir: &Path) -> Vec<LoadedDeclarativeTool> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };

        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        paths.sort();

        paths
            .into_iter()
            .filter_map(|path| match LoadedDeclarativeTool::load(&path) {
                Ok(tool) => Some(tool),
                Err(error) => {
                    tracing::warn!(error = %error, "failed to load declarative custom tool");
                    None
                }
            })
            .collect()
    }

    fn discovered_tool(loaded: LoadedDeclarativeTool) -> DiscoveredTool {
        let spec = Arc::new(DeclarativeToolSpec::from_loaded_tool(&loaded));
        let executor = DeclarativeToolExecutor::from_loaded_tool(Arc::clone(&spec), &loaded);
        DiscoveredTool {
            spec,
            executor: Arc::new(executor),
        }
    }
}

impl ToolSource for PluginToolSource {
    fn discover(&self) -> Vec<DiscoveredTool> {
        let mut discovered_tools = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for dir in self.discovery_dirs() {
            for loaded in Self::discover_dir(&dir) {
                let tool_name = loaded.manifest.name.clone();
                if seen_names.insert(tool_name) {
                    discovered_tools.push(Self::discovered_tool(loaded));
                }
            }
        }

        discovered_tools
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/tool/impl/plugin/tool_source_test.rs"]
mod tests;
