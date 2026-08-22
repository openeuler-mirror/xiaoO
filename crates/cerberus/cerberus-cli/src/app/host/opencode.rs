//! OpenCode host adapter.

use std::path::{Path, PathBuf};

use super::{AdapterStatus, Host, HostAdapter, CERBERUS_MARKER};
use crate::app::error::CliError;

/// OpenCode adapter for Cerberus host scaffolding.
pub struct OpenCodeAdapter {
    config_dir: PathBuf,
}

impl OpenCodeAdapter {
    /// Create a new OpenCode adapter.
    pub fn new() -> Self {
        let config_dir = dirs::home_dir()
            .map(|h| h.join(".config").join("opencode"))
            .unwrap_or_else(|| PathBuf::from(".config/opencode"));
        Self { config_dir }
    }

    /// opencode.json config path.
    fn config_json_path(&self) -> PathBuf {
        self.config_dir.join("opencode.json")
    }

    /// Agent definition path.
    fn agent_path(&self) -> PathBuf {
        self.config_dir.join("agents").join("cerberus.md")
    }

    /// Generate opencode.json content (minimal MCP entry).
    fn config_json_content() -> String {
        serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": {
                "cerberus-proxy": {
                    "type": "stdio",
                    "command": "cerberus-mcp",
                    "args": ["serve"],
                    "enabled": true
                }
            }
        })
        .to_string()
    }

    /// Generate agent definition content.
    fn agent_content() -> String {
        format!(
            r#"{marker}
---
description: Cerberus scaffolding marker
mode: subagent
model: anthropic/claude-sonnet-4-5
permission:
  edit: ask
  bash: ask
---

This file indicates Cerberus host scaffolding is installed.

MCP configuration is registered for potential Cerberus routing.
Use `cerberus init --opencode --show` to verify scaffolding status.
"#,
            marker = CERBERUS_MARKER
        )
    }

    /// Merge Cerberus MCP entry into existing config.
    fn merge_mcp_config(&self, base: &Path, force: bool) -> Result<Vec<String>, CliError> {
        let config_path = base.join("opencode.json");
        let mut actions = Vec::new();

        if !config_path.exists() {
            std::fs::write(&config_path, Self::config_json_content())?;
            actions.push(format!("Created config: {}", config_path.display()));
            return Ok(actions);
        }

        let content = std::fs::read_to_string(&config_path)?;
        let mut config: serde_json::Value = serde_json::from_str(&content)?;

        if let Some(mcp) = config.get("mcp") {
            if mcp.get("cerberus-proxy").is_some() && !force {
                actions.push(format!(
                    "MCP entry already exists in {} (use --force to overwrite)",
                    config_path.display()
                ));
                return Ok(actions);
            }
        }

        if config.get("mcp").is_none() {
            config["mcp"] = serde_json::json!({});
        }

        config["mcp"]["cerberus-proxy"] = serde_json::json!({
            "type": "stdio",
            "command": "cerberus-mcp",
            "args": ["serve"],
            "enabled": true
        });

        super::backup_file(&config_path)?;
        actions.push(format!("Backed up: {}.bak", config_path.display()));

        std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
        actions.push(format!("Updated config: {}", config_path.display()));

        Ok(actions)
    }
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostAdapter for OpenCodeAdapter {
    fn host(&self) -> Host {
        Host::OpenCode
    }

    fn host_binary(&self) -> &'static str {
        "opencode"
    }

    fn config_dir(&self) -> PathBuf {
        self.config_dir.clone()
    }

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.config_json_path(), self.agent_path()]
    }

    fn detect(&self) -> AdapterStatus {
        let mut status = AdapterStatus::new(Host::OpenCode);
        status.host_installed = self.detect_host();

        let config = self.config_json_path();
        let agent = self.agent_path();

        let config_exists = config.exists();
        let agent_exists = agent.exists();

        status.add_file(config.clone(), config_exists, false);
        status.add_file(agent.clone(), agent_exists, true);

        if status.host_installed {
            status.add_message("OpenCode is installed".to_string());
        } else {
            status.add_message("OpenCode is not installed".to_string());
        }

        status.integration_installed = agent_exists;

        if status.integration_installed {
            status.add_message("Cerberus host scaffolding is installed".to_string());
        } else {
            status.add_message("Cerberus host scaffolding is not installed".to_string());
        }

        if config_exists {
            if let Ok(content) = std::fs::read_to_string(&config) {
                if content.contains("cerberus") {
                    status.add_message("MCP entry found in opencode.json".to_string());
                }
            }
        }

        status
    }

    fn install(&self, force: bool, base_path: Option<&PathBuf>) -> Result<Vec<String>, CliError> {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut actions = Vec::new();

        if !base.exists() {
            std::fs::create_dir_all(&base)?;
            actions.push(format!("Created directory: {}", base.display()));
        }

        let agents_dir = base.join("agents");
        if !agents_dir.exists() {
            std::fs::create_dir_all(&agents_dir)?;
            actions.push(format!("Created directory: {}", agents_dir.display()));
        }

        let agent_path = base.join("agents").join("cerberus.md");
        if !agent_path.exists() || force {
            std::fs::write(&agent_path, Self::agent_content())?;
            actions.push(format!(
                "Created agent definition: {}",
                agent_path.display()
            ));
        } else {
            actions.push(format!(
                "Agent definition already exists: {} (use --force to overwrite)",
                agent_path.display()
            ));
        }

        let mcp_actions = self.merge_mcp_config(&base, force)?;
        actions.extend(mcp_actions);

        Ok(actions)
    }

    fn show(&self, base_path: Option<&PathBuf>) -> AdapterStatus {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut status = AdapterStatus::new(Host::OpenCode);
        status.host_installed = self.detect_host();

        let config = base.join("opencode.json");
        let agent = base.join("agents").join("cerberus.md");

        status.add_file(config.clone(), config.exists(), false);
        status.add_file(agent.clone(), agent.exists(), true);

        if status.host_installed {
            status.add_message("OpenCode binary found".to_string());
        } else {
            status.add_message("OpenCode binary not found in PATH".to_string());
        }

        status.integration_installed = agent.exists();

        if status.integration_installed {
            status.add_message("Cerberus agent definition installed".to_string());
        } else {
            status.add_message("Cerberus agent definition not installed".to_string());
        }

        if config.exists() {
            if let Ok(content) = std::fs::read_to_string(&config) {
                if content.contains("cerberus") {
                    status.add_message("MCP entry found in opencode.json".to_string());
                } else {
                    status.add_message("No MCP entry for Cerberus in opencode.json".to_string());
                }
            }
        }

        status
    }

    fn uninstall(&self, base_path: Option<&PathBuf>) -> Result<Vec<String>, CliError> {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut actions = Vec::new();

        let agent_path = base.join("agents").join("cerberus.md");
        if agent_path.exists() {
            let content = std::fs::read_to_string(&agent_path)?;
            if content.contains(CERBERUS_MARKER) {
                std::fs::remove_file(&agent_path)?;
                actions.push(format!("Removed: {}", agent_path.display()));
            } else {
                actions.push(format!(
                    "Skipped {} (not managed by Cerberus)",
                    agent_path.display()
                ));
            }
        }

        let config_path = base.join("opencode.json");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let mut config: serde_json::Value = serde_json::from_str(&content)?;

            if let Some(mcp) = config.get("mcp") {
                if mcp.get("cerberus-proxy").is_some() {
                    super::backup_file(&config_path)?;
                    actions.push(format!("Backed up: {}.bak", config_path.display()));

                    if let Some(mcp_obj) = config.get_mut("mcp") {
                        if let Some(mcp_map) = mcp_obj.as_object_mut() {
                            mcp_map.remove("cerberus-proxy");
                        }
                    }

                    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
                    actions.push(format!("Removed MCP entry from: {}", config_path.display()));
                }
            }
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_opencode_adapter_paths() {
        let adapter = OpenCodeAdapter::new();
        assert!(adapter.config_dir().to_string_lossy().contains("opencode"));
        assert!(adapter
            .config_json_path()
            .to_string_lossy()
            .contains("opencode.json"));
        assert!(adapter
            .agent_path()
            .to_string_lossy()
            .contains("cerberus.md"));
    }

    #[test]
    fn test_opencode_adapter_detect() {
        let adapter = OpenCodeAdapter::new();
        let status = adapter.detect();
        assert_eq!(status.host, Host::OpenCode);
    }

    #[test]
    fn test_opencode_adapter_install_uninstall() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = OpenCodeAdapter::new();

        let actions = adapter.install(false, Some(&base)).unwrap();
        assert!(actions
            .iter()
            .any(|a| a.contains("Created agent definition")));
        assert!(actions.iter().any(|a| a.contains("config")));

        let agent_path = base.join("agents").join("cerberus.md");
        assert!(agent_path.exists());

        let config_path = base.join("opencode.json");
        assert!(config_path.exists());

        let actions = adapter.uninstall(Some(&base)).unwrap();
        assert!(actions.iter().any(|a| a.contains("Removed")));
        assert!(!agent_path.exists());
    }

    #[test]
    fn test_opencode_mcp_merge() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = OpenCodeAdapter::new();

        let existing_config = serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": {
                "existing-server": {
                    "type": "stdio",
                    "command": "some-command"
                }
            }
        });
        std::fs::write(
            base.join("opencode.json"),
            serde_json::to_string(&existing_config).unwrap(),
        )
        .unwrap();

        let _actions = adapter.install(false, Some(&base)).unwrap();

        let content = std::fs::read_to_string(base.join("opencode.json")).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(config["mcp"]["existing-server"].is_object());
        assert!(config["mcp"]["cerberus-proxy"].is_object());
    }
}
