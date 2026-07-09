use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::input::Input;

const REGISTRY_VERSION: u32 = 1;
const MAX_REMOTE_SESSIONS: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSessionRecord {
    pub session_id: String,
    pub base_url: String,
    #[serde(default)]
    pub bearer_token_env: Option<String>,
    #[serde(default)]
    pub first_message_preview: Option<String>,
    pub created_at_ms: u64,
    pub last_active_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteSessionRegistry {
    version: u32,
    #[serde(default)]
    sessions: Vec<RemoteSessionRecord>,
}

impl Default for RemoteSessionRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            sessions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RemoteSessionDialogEntry {
    Existing(RemoteSessionRecord),
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSessionDialogMode {
    List,
    NewUrl,
}

#[derive(Debug, Clone)]
pub struct RemoteSessionDialog {
    pub entries: Vec<RemoteSessionDialogEntry>,
    pub selected: usize,
    pub mode: RemoteSessionDialogMode,
    pub url_input: Input,
    pub error: Option<String>,
    /// When `true`, the dialog is used for switching between sessions on the
    /// currently connected daemon: the "New remote session..." entry is
    /// hidden and the row matching `current_session_id` is marked.
    pub switch_only: bool,
    /// Session id currently active on the connected daemon, if any. Used to
    /// highlight the active row in switch mode.
    pub current_session_id: Option<String>,
}

impl RemoteSessionDialog {
    pub fn new(records: Vec<RemoteSessionRecord>) -> Self {
        let mut entries = records
            .into_iter()
            .map(RemoteSessionDialogEntry::Existing)
            .collect::<Vec<_>>();
        entries.push(RemoteSessionDialogEntry::New);
        Self {
            entries,
            selected: 0,
            mode: RemoteSessionDialogMode::List,
            url_input: Input::default(),
            error: None,
            switch_only: false,
            current_session_id: None,
        }
    }

    /// Build a switch-mode dialog. `records` should already be filtered to
    /// the daemon the user is currently connected to. The "New remote
    /// session..." entry is omitted; `current_session_id` is highlighted
    /// with a marker and pre-selected so the next/previous row is the
    /// nearest alternative.
    pub fn new_for_switch(
        records: Vec<RemoteSessionRecord>,
        current_session_id: Option<String>,
    ) -> Self {
        let entries = records
            .into_iter()
            .map(RemoteSessionDialogEntry::Existing)
            .collect::<Vec<_>>();
        // Default cursor to the first row that is NOT the current session, so
        // pressing Enter immediately switches to a different session. Falls
        // back to 0 when there is no current-session match (or only one
        // session exists).
        let selected = match &current_session_id {
            Some(id) => entries
                .iter()
                .position(|entry| match entry {
                    RemoteSessionDialogEntry::Existing(record) => record.session_id == *id,
                    _ => false,
                })
                .map(|idx| if idx + 1 < entries.len() { idx + 1 } else { 0 })
                .unwrap_or(0),
            None => 0,
        };
        Self {
            entries,
            selected,
            mode: RemoteSessionDialogMode::List,
            url_input: Input::default(),
            error: None,
            switch_only: true,
            current_session_id,
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1).min(self.entries.len() - 1);
        }
    }

    pub fn selected_entry(&self) -> Option<&RemoteSessionDialogEntry> {
        self.entries.get(self.selected)
    }

    pub fn enter_new_url_mode(&mut self) {
        self.mode = RemoteSessionDialogMode::NewUrl;
        self.error = None;
    }
}

pub fn list_remote_sessions() -> Result<Vec<RemoteSessionRecord>> {
    let mut registry = load_registry()?;
    registry.sessions.sort_by(|left, right| {
        right
            .last_active_at_ms
            .cmp(&left.last_active_at_ms)
            .then_with(|| left.base_url.cmp(&right.base_url))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(registry.sessions)
}

/// Public wrapper around the same normalization the registry uses when
/// storing records. Callers (e.g. the `/sessions` switch dialog) need it to
/// match `RemoteSessionRecord.base_url` regardless of trailing slashes the
/// daemon URL was configured with.
pub fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

pub fn record_remote_session(
    session_id: &str,
    base_url: &str,
    bearer_token_env: Option<String>,
    first_message: Option<&str>,
) -> Result<RemoteSessionRecord> {
    let now = current_time_ms();
    let mut registry = load_registry()?;
    let normalized_url = normalize_base_url(base_url);
    let first_message_preview = first_message
        .map(summarize_first_message)
        .filter(|value| !value.is_empty());

    let record = match registry.sessions.iter_mut().find(|record| {
        record.session_id == session_id && normalize_base_url(&record.base_url) == normalized_url
    }) {
        Some(record) => {
            record.base_url = normalized_url.clone();
            record.bearer_token_env = bearer_token_env;
            if record.first_message_preview.is_none() {
                record.first_message_preview = first_message_preview;
            }
            record.last_active_at_ms = now;
            record.clone()
        }
        None => {
            let record = RemoteSessionRecord {
                session_id: session_id.to_string(),
                base_url: normalized_url,
                bearer_token_env,
                first_message_preview,
                created_at_ms: now,
                last_active_at_ms: now,
            };
            registry.sessions.push(record.clone());
            record
        }
    };

    registry.sessions.sort_by(|left, right| {
        right
            .last_active_at_ms
            .cmp(&left.last_active_at_ms)
            .then_with(|| left.base_url.cmp(&right.base_url))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    registry.sessions.truncate(MAX_REMOTE_SESSIONS);
    save_registry(&registry)?;
    Ok(record)
}

pub fn daemon_display(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    if host_port.is_empty() {
        trimmed.to_string()
    } else {
        host_port.to_string()
    }
}

pub fn format_remote_time(timestamp_ms: u64) -> String {
    match Local.timestamp_millis_opt(timestamp_ms as i64).single() {
        Some(dt) => dt.format("%m-%d %H:%M").to_string(),
        None => "-".to_string(),
    }
}

pub fn summarize_first_message(text: &str) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&flattened, 36)
}

fn load_registry() -> Result<RemoteSessionRegistry> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(RemoteSessionRegistry::default());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read remote session registry {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(RemoteSessionRegistry::default());
    }
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse remote session registry {}", path.display()))
}

fn save_registry(registry: &RemoteSessionRegistry) -> Result<()> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create remote session dir {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(registry)?;
    fs::write(&path, content)
        .with_context(|| format!("failed to write remote session registry {}", path.display()))
}

fn registry_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("unable to resolve home directory for ~/.xiaoo")?;
    Ok(home.join(".xiaoo").join("remote_sessions.json"))
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let prefix: String = value.chars().take(max_chars - 3).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::{daemon_display, summarize_first_message};

    #[test]
    fn daemon_display_extracts_host_and_port() {
        assert_eq!(daemon_display("http://127.0.0.1:8070/"), "127.0.0.1:8070");
        assert_eq!(
            daemon_display("https://example.com:443/api"),
            "example.com:443"
        );
    }

    #[test]
    fn first_message_summary_is_one_line() {
        assert_eq!(
            summarize_first_message("hello\nremote   session"),
            "hello remote session"
        );
        assert!(summarize_first_message(&"a".repeat(80)).ends_with("..."));
    }
}
