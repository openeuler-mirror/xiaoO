//! Host adapter modules for Cerberus CLI host scaffolding.
//!
//! This module provides thin adapters for installing Cerberus host scaffolding for
//! various AI agent hosts (Claude Code, Codex CLI, OpenCode).

pub mod claude;
pub mod codex;
pub mod opencode;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Supported host types.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum Host {
    /// Claude Code (Anthropic)
    Claude,
    /// Codex CLI (OpenAI)
    Codex,
    /// OpenCode (Anomaly)
    OpenCode,
}

/// Action to perform on the host adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Install integration files.
    Install,
    /// Show current integration status.
    Show,
    /// Remove integration files.
    Uninstall,
}

/// Status of a single integration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatus {
    /// File path.
    pub path: PathBuf,
    /// Whether the file exists.
    pub exists: bool,
    /// Whether it was created by Cerberus.
    pub cerberus_owned: bool,
}

/// Overall status of host integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterStatus {
    /// The host type.
    pub host: Host,
    /// Whether the host tool is installed on the system.
    pub host_installed: bool,
    /// Whether Cerberus host scaffolding is installed.
    pub integration_installed: bool,
    /// Status of individual files.
    pub files: Vec<FileStatus>,
    /// Diagnostic messages.
    pub messages: Vec<String>,
}

impl AdapterStatus {
    /// Create a new status for the given host.
    pub fn new(host: Host) -> Self {
        Self {
            host,
            host_installed: false,
            integration_installed: false,
            files: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Add a file status entry.
    pub fn add_file(&mut self, path: PathBuf, exists: bool, cerberus_owned: bool) {
        self.files.push(FileStatus {
            path,
            exists,
            cerberus_owned,
        });
    }

    /// Add a diagnostic message.
    pub fn add_message(&mut self, msg: impl Into<String>) {
        self.messages.push(msg.into());
    }
}

/// Trait for host adapters.
pub trait HostAdapter: Send + Sync {
    /// Returns the host type this adapter handles.
    fn host(&self) -> Host;

    /// Returns the name of the host tool (for `which` lookup).
    fn host_binary(&self) -> &'static str;

    /// Detects if the host is installed on the system.
    fn detect_host(&self) -> bool {
        detect_binary(self.host_binary())
    }

    /// Returns the base config directory for the host.
    fn config_dir(&self) -> PathBuf;

    /// Returns the paths managed by this adapter.
    fn managed_paths(&self) -> Vec<PathBuf>;

    /// Detects current integration status.
    fn detect(&self) -> AdapterStatus;

    /// Installs integration files.
    fn install(
        &self,
        force: bool,
        base_path: Option<&PathBuf>,
    ) -> Result<Vec<String>, crate::app::error::CliError>;

    /// Shows detailed status information.
    fn show(&self, base_path: Option<&PathBuf>) -> AdapterStatus;

    /// Uninstalls integration files.
    fn uninstall(
        &self,
        base_path: Option<&PathBuf>,
    ) -> Result<Vec<String>, crate::app::error::CliError>;
}

/// Create an adapter for the given host.
pub fn create_adapter(host: Host) -> Box<dyn HostAdapter> {
    match host {
        Host::Claude => Box::new(claude::ClaudeAdapter::new()),
        Host::Codex => Box::new(codex::CodexAdapter::new()),
        Host::OpenCode => Box::new(opencode::OpenCodeAdapter::new()),
    }
}

/// Helper to get the effective base path (for testing injection).
pub fn effective_base(base: Option<&PathBuf>, default: PathBuf) -> PathBuf {
    base.cloned().unwrap_or(default)
}

/// Helper to backup a file before modification.
pub fn backup_file(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let backup = path.with_extension(format!(
        "{}.bak",
        path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
    ));
    std::fs::copy(path, &backup)?;
    Ok(Some(backup))
}

/// Helper to create parent directories if they don't exist.
pub fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// Detect if a binary exists in PATH.
pub fn detect_binary(name: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for path_dir in path_var.split(':') {
            let binary_path = std::path::Path::new(path_dir).join(name);
            if binary_path.exists() && binary_path.is_file() {
                return true;
            }
        }
    }
    false
}

/// Marker content to identify Cerberus-managed files.
pub const CERBERUS_MARKER: &str = "# Cerberus Integration - Managed by cerberus init";
