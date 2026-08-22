//! Codex CLI host adapter.

use std::path::{Path, PathBuf};

use super::{AdapterStatus, Host, HostAdapter, CERBERUS_MARKER};
use crate::app::error::CliError;

/// Codex CLI adapter for Cerberus host scaffolding.
pub struct CodexAdapter {
    config_dir: PathBuf,
}

impl CodexAdapter {
    /// Create a new Codex adapter.
    pub fn new() -> Self {
        let config_dir = dirs::home_dir()
            .map(|h| h.join(".codex"))
            .unwrap_or_else(|| PathBuf::from(".codex"));
        Self { config_dir }
    }

    /// Hooks.json path.
    fn hooks_json_path(&self) -> PathBuf {
        self.config_dir.join("hooks.json")
    }

    /// AGENTS.md path.
    fn agents_md_path(&self) -> PathBuf {
        self.config_dir.join("AGENTS.md")
    }

    /// Generate hooks.json content.
    fn hooks_json_content() -> String {
        serde_json::json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "cerberus exec",
                        "statusMessage": "Cerberus hook configured"
                    }]
                }]
            }
        })
        .to_string()
    }

    /// Generate AGENTS.md content.
    fn agents_md_content() -> String {
        format!(
            r#"{marker}

# Cerberus Scaffolding

This file indicates Cerberus host scaffolding is installed.

Hooks configuration is registered for potential Cerberus routing.
Use `cerberus init --codex --show` to verify scaffolding status.
"#,
            marker = CERBERUS_MARKER
        )
    }

    /// Check if hooks are enabled in config.toml.
    fn hooks_enabled(&self, base: &Path) -> bool {
        let config_path = base.join("config.toml");
        if !config_path.exists() {
            return false;
        }
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            return content.contains("codex_hooks = true") || content.contains("codex_hooks=true");
        }
        false
    }

    /// Enable hooks in config.toml by adding/updating the feature flag.
    fn enable_hooks(&self, base: &Path) -> Result<Vec<String>, CliError> {
        let config_path = base.join("config.toml");
        let mut actions = Vec::new();

        if !config_path.exists() {
            let minimal_config = r#"# Codex configuration
[features]
codex_hooks = true
"#;
            std::fs::write(&config_path, minimal_config)?;
            actions.push(format!("Created config: {}", config_path.display()));
            return Ok(actions);
        }

        let content = std::fs::read_to_string(&config_path)?;

        if content.contains("codex_hooks = true") || content.contains("codex_hooks=true") {
            actions.push(format!(
                "Hooks already enabled in: {}",
                config_path.display()
            ));
            return Ok(actions);
        }

        super::backup_file(&config_path)?;
        actions.push(format!("Backed up: {}.bak", config_path.display()));

        let updated = if content.contains("[features]") {
            content.replace("[features]", "[features]\ncodex_hooks = true")
        } else {
            format!("{}\n\n[features]\ncodex_hooks = true\n", content.trim_end())
        };

        std::fs::write(&config_path, updated)?;
        actions.push(format!("Enabled hooks in: {}", config_path.display()));

        Ok(actions)
    }

    /// Remove hooks feature flag from config.toml.
    fn disable_hooks(&self, base: &Path) -> Result<Vec<String>, CliError> {
        let config_path = base.join("config.toml");
        let mut actions = Vec::new();

        if !config_path.exists() {
            return Ok(actions);
        }

        let content = std::fs::read_to_string(&config_path)?;

        if !content.contains("codex_hooks") {
            return Ok(actions);
        }

        super::backup_file(&config_path)?;
        actions.push(format!("Backed up: {}.bak", config_path.display()));

        let lines: Vec<&str> = content.lines().collect();
        let filtered: Vec<&str> = lines
            .iter()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed != "codex_hooks = true"
                    && trimmed != "codex_hooks=true"
                    && trimmed != "codex_hooks = false"
                    && trimmed != "codex_hooks=false"
            })
            .copied()
            .collect();

        let updated = filtered.join("\n");
        std::fs::write(&config_path, updated)?;
        actions.push(format!("Disabled hooks in: {}", config_path.display()));

        Ok(actions)
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostAdapter for CodexAdapter {
    fn host(&self) -> Host {
        Host::Codex
    }

    fn host_binary(&self) -> &'static str {
        "codex"
    }

    fn config_dir(&self) -> PathBuf {
        self.config_dir.clone()
    }

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.hooks_json_path(), self.agents_md_path()]
    }

    fn detect(&self) -> AdapterStatus {
        let mut status = AdapterStatus::new(Host::Codex);
        status.host_installed = self.detect_host();

        let hooks = self.hooks_json_path();
        let agents = self.agents_md_path();

        let hooks_exists = hooks.exists();
        let agents_exists = agents.exists();

        status.add_file(hooks.clone(), hooks_exists, true);
        status.add_file(agents.clone(), agents_exists, true);

        if status.host_installed {
            status.add_message("Codex CLI is installed".to_string());
        } else {
            status.add_message("Codex CLI is not installed".to_string());
        }

        status.integration_installed = hooks_exists && agents_exists;

        if status.integration_installed {
            status.add_message("Cerberus host scaffolding is installed".to_string());
            if self.hooks_enabled(&self.config_dir) {
                status.add_message("Hooks are enabled in config.toml".to_string());
            } else {
                status.add_message(
                    "Note: codex_hooks may need to be enabled in config.toml".to_string(),
                );
            }
        } else {
            status.add_message("Cerberus host scaffolding is not installed".to_string());
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

        let hooks_path = base.join("hooks.json");
        if !hooks_path.exists() || force {
            if hooks_path.exists() && force {
                super::backup_file(&hooks_path)?;
                actions.push(format!("Backed up: {}.bak", hooks_path.display()));
            }
            std::fs::write(&hooks_path, Self::hooks_json_content())?;
            actions.push(format!("Created hooks config: {}", hooks_path.display()));
        } else {
            actions.push(format!(
                "Hooks config already exists: {} (use --force to overwrite)",
                hooks_path.display()
            ));
        }

        let agents_path = base.join("AGENTS.md");
        if !agents_path.exists() || force {
            std::fs::write(&agents_path, Self::agents_md_content())?;
            actions.push(format!(
                "Created instruction file: {}",
                agents_path.display()
            ));
        } else {
            actions.push(format!(
                "AGENTS.md already exists: {} (use --force to overwrite)",
                agents_path.display()
            ));
        }

        let config_actions = self.enable_hooks(&base)?;
        actions.extend(config_actions);

        Ok(actions)
    }

    fn show(&self, base_path: Option<&PathBuf>) -> AdapterStatus {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut status = AdapterStatus::new(Host::Codex);
        status.host_installed = self.detect_host();

        let hooks = base.join("hooks.json");
        let agents = base.join("AGENTS.md");
        let config = base.join("config.toml");

        status.add_file(hooks.clone(), hooks.exists(), true);
        status.add_file(agents.clone(), agents.exists(), true);
        status.add_file(config.clone(), config.exists(), false);

        if status.host_installed {
            status.add_message("Codex CLI binary found".to_string());
        } else {
            status.add_message("Codex CLI binary not found in PATH".to_string());
        }

        status.integration_installed = hooks.exists() && agents.exists();

        if status.integration_installed {
            status.add_message("Cerberus host scaffolding installed".to_string());
            if self.hooks_enabled(&base) {
                status.add_message("Hooks are enabled".to_string());
            } else {
                status.add_message(
                    "Hooks feature not enabled (add codex_hooks = true to config.toml)".to_string(),
                );
            }
        } else {
            status.add_message("Cerberus host scaffolding not installed".to_string());
        }

        status
    }

    fn uninstall(&self, base_path: Option<&PathBuf>) -> Result<Vec<String>, CliError> {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut actions = Vec::new();

        let hooks_path = base.join("hooks.json");
        if hooks_path.exists() {
            let content = std::fs::read_to_string(&hooks_path)?;
            if content.contains("cerberus") {
                std::fs::remove_file(&hooks_path)?;
                actions.push(format!("Removed: {}", hooks_path.display()));
            } else {
                actions.push(format!(
                    "Skipped {} (not a Cerberus hook config)",
                    hooks_path.display()
                ));
            }
        }

        let agents_path = base.join("AGENTS.md");
        if agents_path.exists() {
            let content = std::fs::read_to_string(&agents_path)?;
            if content.contains(CERBERUS_MARKER) {
                std::fs::remove_file(&agents_path)?;
                actions.push(format!("Removed: {}", agents_path.display()));
            } else {
                actions.push(format!(
                    "Skipped {} (not managed by Cerberus)",
                    agents_path.display()
                ));
            }
        }

        let config_actions = self.disable_hooks(&base)?;
        actions.extend(config_actions);

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_codex_adapter_paths() {
        let adapter = CodexAdapter::new();
        assert!(adapter.config_dir().to_string_lossy().contains(".codex"));
        assert!(adapter
            .hooks_json_path()
            .to_string_lossy()
            .contains("hooks.json"));
        assert!(adapter
            .agents_md_path()
            .to_string_lossy()
            .contains("AGENTS.md"));
    }

    #[test]
    fn test_codex_adapter_detect() {
        let adapter = CodexAdapter::new();
        let status = adapter.detect();
        assert_eq!(status.host, Host::Codex);
    }

    #[test]
    fn test_codex_adapter_install_uninstall() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = CodexAdapter::new();

        let actions = adapter.install(false, Some(&base)).unwrap();
        assert!(actions.iter().any(|a| a.contains("Created hooks config")));
        assert!(actions
            .iter()
            .any(|a| a.contains("Created instruction file")));

        let hooks_path = base.join("hooks.json");
        assert!(hooks_path.exists());

        let agents_path = base.join("AGENTS.md");
        assert!(agents_path.exists());

        let actions = adapter.uninstall(Some(&base)).unwrap();
        assert!(actions.iter().any(|a| a.contains("Removed")));
        assert!(!hooks_path.exists());
    }

    #[test]
    fn test_codex_hooks_enabled() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = CodexAdapter::new();

        assert!(!adapter.hooks_enabled(&base));

        std::fs::write(base.join("config.toml"), "codex_hooks = true").unwrap();
        assert!(adapter.hooks_enabled(&base));
    }

    #[test]
    fn test_codex_enable_hooks_create() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = CodexAdapter::new();

        let actions = adapter.enable_hooks(&base).unwrap();
        assert!(actions.iter().any(|a| a.contains("Created config")));

        let config_path = base.join("config.toml");
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("codex_hooks = true"));
    }

    #[test]
    fn test_codex_enable_hooks_existing() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = CodexAdapter::new();

        std::fs::write(
            base.join("config.toml"),
            "# Codex config\nmodel = 'o4-mini'",
        )
        .unwrap();

        let actions = adapter.enable_hooks(&base).unwrap();
        assert!(actions.iter().any(|a| a.contains("Enabled hooks")));

        let content = std::fs::read_to_string(base.join("config.toml")).unwrap();
        assert!(content.contains("model = 'o4-mini'"));
        assert!(content.contains("codex_hooks = true"));
    }

    #[test]
    fn test_codex_disable_hooks() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = CodexAdapter::new();

        adapter.enable_hooks(&base).unwrap();

        let actions = adapter.disable_hooks(&base).unwrap();
        assert!(actions.iter().any(|a| a.contains("Disabled hooks")));

        let content = std::fs::read_to_string(base.join("config.toml")).unwrap();
        assert!(!content.contains("codex_hooks"));
    }
}
