//! Claude Code host adapter.

use std::path::{Path, PathBuf};

use super::{AdapterStatus, Host, HostAdapter, CERBERUS_MARKER};
use crate::app::error::CliError;

/// Claude Code adapter for Cerberus host scaffolding.
pub struct ClaudeAdapter {
    config_dir: PathBuf,
}

impl ClaudeAdapter {
    /// Create a new Claude adapter.
    pub fn new() -> Self {
        let config_dir = dirs::home_dir()
            .map(|h| h.join(".claude"))
            .unwrap_or_else(|| PathBuf::from(".claude"));
        Self { config_dir }
    }

    /// Hook script path.
    fn hook_path(&self) -> PathBuf {
        self.config_dir.join("hooks").join("cerberus-filter.sh")
    }

    /// CERBERUS.md instruction file path.
    fn instruction_path(&self) -> PathBuf {
        self.config_dir.join("CERBERUS.md")
    }

    /// Generate hook script content.
    fn hook_content() -> &'static str {
        include_str!("../../templates/claude-hook.sh")
    }

    /// Generate instruction file content.
    fn instruction_content() -> String {
        format!(
            r#"{marker}

# Cerberus Scaffolding

This file indicates Cerberus host scaffolding is installed.

Hook scripts are configured in settings.json for potential Cerberus routing.
Use `cerberus init --claude --show` to verify scaffolding status.
"#,
            marker = CERBERUS_MARKER
        )
    }

    /// Merge hook registration into settings.json.
    fn merge_settings(&self, base: &Path, force: bool) -> Result<Vec<String>, CliError> {
        let settings_path = base.join("settings.json");
        let mut actions = Vec::new();

        let hook_path_str = base
            .join("hooks")
            .join("cerberus-filter.sh")
            .to_string_lossy()
            .to_string();

        if !settings_path.exists() {
            let minimal_settings = serde_json::json!({
                "hooks": {
                    "PostToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": hook_path_str
                        }]
                    }]
                }
            });
            std::fs::write(
                &settings_path,
                serde_json::to_string_pretty(&minimal_settings)?,
            )?;
            actions.push(format!("Created settings: {}", settings_path.display()));
            return Ok(actions);
        }

        let content = std::fs::read_to_string(&settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&content)?;

        if settings.get("hooks").is_none() {
            settings["hooks"] = serde_json::json!({});
        }

        if let Some(hooks) = settings.get("hooks") {
            if let Some(post_tool_use) = hooks.get("PostToolUse") {
                if let Some(arr) = post_tool_use.as_array() {
                    for entry in arr {
                        if let Some(matcher) = entry.get("matcher") {
                            if matcher.as_str() == Some("Bash") {
                                if let Some(hooks_arr) =
                                    entry.get("hooks").and_then(|h| h.as_array())
                                {
                                    for hook in hooks_arr {
                                        if !force
                                            && hook.get("command").is_some_and(|cmd| {
                                                cmd.as_str().is_some_and(|s| {
                                                    s.contains("cerberus-filter.sh")
                                                })
                                            })
                                        {
                                            actions.push(format!(
                                                "Hook already registered in: {}",
                                                settings_path.display()
                                            ));
                                            return Ok(actions);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let new_entry = serde_json::json!({
            "matcher": "Bash",
            "hooks": [{
                "type": "command",
                "command": hook_path_str
            }]
        });

        if settings["hooks"]["PostToolUse"].is_null() {
            settings["hooks"]["PostToolUse"] = serde_json::json!([]);
        }

        if let Some(post_tool_use) = settings["hooks"]["PostToolUse"].as_array_mut() {
            post_tool_use.push(new_entry);
        }

        super::backup_file(&settings_path)?;
        actions.push(format!("Backed up: {}.bak", settings_path.display()));

        std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
        actions.push(format!("Updated settings: {}", settings_path.display()));

        Ok(actions)
    }

    /// Remove hook registration from settings.json.
    fn remove_from_settings(&self, base: &Path) -> Result<Vec<String>, CliError> {
        let settings_path = base.join("settings.json");
        let mut actions = Vec::new();

        if !settings_path.exists() {
            return Ok(actions);
        }

        let content = std::fs::read_to_string(&settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&content)?;

        let mut modified = false;

        if let Some(hooks) = settings.get_mut("hooks") {
            if let Some(post_tool_use) = hooks.get_mut("PostToolUse") {
                if let Some(arr) = post_tool_use.as_array_mut() {
                    let original_len = arr.len();
                    arr.retain(|entry| {
                        if let Some(hooks_arr) = entry.get("hooks").and_then(|h| h.as_array()) {
                            for hook in hooks_arr {
                                if let Some(cmd) = hook.get("command") {
                                    if cmd
                                        .as_str()
                                        .map(|s| s.contains("cerberus-filter.sh"))
                                        .unwrap_or(false)
                                    {
                                        return false;
                                    }
                                }
                            }
                        }
                        true
                    });
                    if arr.len() != original_len {
                        modified = true;
                    }
                }
            }
        }

        if modified {
            super::backup_file(&settings_path)?;
            actions.push(format!("Backed up: {}.bak", settings_path.display()));
            std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
            actions.push(format!(
                "Removed hook registration from: {}",
                settings_path.display()
            ));
        }

        Ok(actions)
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostAdapter for ClaudeAdapter {
    fn host(&self) -> Host {
        Host::Claude
    }

    fn host_binary(&self) -> &'static str {
        "claude"
    }

    fn config_dir(&self) -> PathBuf {
        self.config_dir.clone()
    }

    fn managed_paths(&self) -> Vec<PathBuf> {
        vec![self.hook_path(), self.instruction_path()]
    }

    fn detect(&self) -> AdapterStatus {
        let mut status = AdapterStatus::new(Host::Claude);
        status.host_installed = self.detect_host();

        let hook = self.hook_path();
        let instruction = self.instruction_path();

        let hook_exists = hook.exists();
        let instruction_exists = instruction.exists();

        status.add_file(hook.clone(), hook_exists, true);
        status.add_file(instruction.clone(), instruction_exists, true);

        if status.host_installed {
            status.add_message("Claude Code is installed".to_string());
        } else {
            status.add_message("Claude Code is not installed".to_string());
        }

        status.integration_installed = hook_exists && instruction_exists;

        if status.integration_installed {
            status.add_message("Cerberus host scaffolding is installed".to_string());
        } else {
            status.add_message("Cerberus host scaffolding is not installed".to_string());
        }

        status
    }

    fn install(&self, force: bool, base_path: Option<&PathBuf>) -> Result<Vec<String>, CliError> {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut actions = Vec::new();

        let hook_dir = base.join("hooks");
        if !hook_dir.exists() {
            std::fs::create_dir_all(&hook_dir)?;
            actions.push(format!("Created directory: {}", hook_dir.display()));
        }

        let hook_path = base.join("hooks").join("cerberus-filter.sh");
        if !hook_path.exists() || force {
            super::ensure_parent_dir(&hook_path)?;
            std::fs::write(&hook_path, Self::hook_content())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))?;
            }
            actions.push(format!("Created hook: {}", hook_path.display()));
        } else {
            actions.push(format!(
                "Hook already exists: {} (use --force to overwrite)",
                hook_path.display()
            ));
        }

        let instruction_path = base.join("CERBERUS.md");
        if !instruction_path.exists() || force {
            std::fs::write(&instruction_path, Self::instruction_content())?;
            actions.push(format!(
                "Created instruction file: {}",
                instruction_path.display()
            ));
        } else {
            actions.push(format!(
                "Instruction file already exists: {} (use --force to overwrite)",
                instruction_path.display()
            ));
        }

        let settings_actions = self.merge_settings(&base, force)?;
        actions.extend(settings_actions);

        Ok(actions)
    }

    fn show(&self, base_path: Option<&PathBuf>) -> AdapterStatus {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut status = AdapterStatus::new(Host::Claude);
        status.host_installed = self.detect_host();

        let hook = base.join("hooks").join("cerberus-filter.sh");
        let instruction = base.join("CERBERUS.md");
        let settings = base.join("settings.json");
        let claude_md = base.join("CLAUDE.md");

        status.add_file(hook.clone(), hook.exists(), true);
        status.add_file(instruction.clone(), instruction.exists(), true);
        status.add_file(settings.clone(), settings.exists(), false);
        status.add_file(claude_md.clone(), claude_md.exists(), false);

        if status.host_installed {
            status.add_message("Claude Code binary found".to_string());
        } else {
            status.add_message("Claude Code binary not found in PATH".to_string());
        }

        status.integration_installed = hook.exists() && instruction.exists();

        if status.integration_installed {
            status.add_message("Cerberus host scaffolding installed".to_string());

            if settings.exists() {
                if let Ok(content) = std::fs::read_to_string(&settings) {
                    if content.contains("cerberus-filter.sh") {
                        status.add_message("Hook registered in settings.json".to_string());
                    } else {
                        status.add_message("Hook not registered in settings.json".to_string());
                    }
                }
            }
        } else {
            status.add_message("Cerberus host scaffolding not installed".to_string());
        }

        status
    }

    fn uninstall(&self, base_path: Option<&PathBuf>) -> Result<Vec<String>, CliError> {
        let base = super::effective_base(base_path, self.config_dir.clone());
        let mut actions = Vec::new();

        let hook_path = base.join("hooks").join("cerberus-filter.sh");
        if hook_path.exists() {
            std::fs::remove_file(&hook_path)?;
            actions.push(format!("Removed: {}", hook_path.display()));
        }

        let instruction_path = base.join("CERBERUS.md");
        if instruction_path.exists() {
            let content = std::fs::read_to_string(&instruction_path)?;
            if content.contains(CERBERUS_MARKER) {
                std::fs::remove_file(&instruction_path)?;
                actions.push(format!("Removed: {}", instruction_path.display()));
            } else {
                actions.push(format!(
                    "Skipped {} (not managed by Cerberus)",
                    instruction_path.display()
                ));
            }
        }

        let settings_actions = self.remove_from_settings(&base)?;
        actions.extend(settings_actions);

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_claude_adapter_paths() {
        let adapter = ClaudeAdapter::new();
        assert!(adapter.config_dir().to_string_lossy().contains(".claude"));
        assert!(adapter
            .hook_path()
            .to_string_lossy()
            .contains("cerberus-filter.sh"));
        assert!(adapter
            .instruction_path()
            .to_string_lossy()
            .contains("CERBERUS.md"));
    }

    #[test]
    fn test_claude_adapter_detect() {
        let adapter = ClaudeAdapter::new();
        let status = adapter.detect();
        assert_eq!(status.host, Host::Claude);
    }

    #[test]
    fn test_claude_adapter_install_uninstall() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = ClaudeAdapter::new();

        let actions = adapter.install(false, Some(&base)).unwrap();
        assert!(actions.iter().any(|a| a.contains("Created hook")));
        assert!(actions
            .iter()
            .any(|a| a.contains("Created instruction file")));

        let hook_path = base.join("hooks").join("cerberus-filter.sh");
        assert!(hook_path.exists());

        let instruction_path = base.join("CERBERUS.md");
        assert!(instruction_path.exists());

        let actions = adapter.uninstall(Some(&base)).unwrap();
        assert!(actions.iter().any(|a| a.contains("Removed")));
        assert!(!hook_path.exists());
    }

    #[test]
    fn test_claude_adapter_idempotent_install() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = ClaudeAdapter::new();

        adapter.install(false, Some(&base)).unwrap();
        let actions = adapter.install(false, Some(&base)).unwrap();

        assert!(actions.iter().any(|a| a.contains("already exists")));
    }

    #[test]
    fn test_claude_adapter_force_install() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = ClaudeAdapter::new();

        adapter.install(false, Some(&base)).unwrap();
        let actions = adapter.install(true, Some(&base)).unwrap();

        assert!(actions.iter().any(|a| a.contains("Created hook")));
    }

    #[test]
    fn test_claude_settings_merge_create() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = ClaudeAdapter::new();

        let actions = adapter.merge_settings(&base, false).unwrap();
        assert!(actions.iter().any(|a| a.contains("Created settings")));

        let settings_path = base.join("settings.json");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        assert!(content.contains("cerberus-filter.sh"));
    }

    #[test]
    fn test_claude_settings_merge_existing() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = ClaudeAdapter::new();

        let existing = serde_json::json!({
            "permissions": {
                "allow": ["Bash(*)"]
            }
        });
        std::fs::write(
            base.join("settings.json"),
            serde_json::to_string(&existing).unwrap(),
        )
        .unwrap();

        let actions = adapter.merge_settings(&base, false).unwrap();
        assert!(actions.iter().any(|a| a.contains("Updated settings")));

        let content = std::fs::read_to_string(base.join("settings.json")).unwrap();
        assert!(content.contains("permissions"));
        assert!(content.contains("cerberus-filter.sh"));
    }

    #[test]
    fn test_claude_settings_remove() {
        let dir = tempdir().unwrap();
        let base = PathBuf::from(dir.path());
        let adapter = ClaudeAdapter::new();

        adapter.merge_settings(&base, false).unwrap();

        let actions = adapter.remove_from_settings(&base).unwrap();
        assert!(actions
            .iter()
            .any(|a| a.contains("Removed hook registration")));

        let content = std::fs::read_to_string(base.join("settings.json")).unwrap();
        assert!(!content.contains("cerberus-filter.sh"));
    }
}
