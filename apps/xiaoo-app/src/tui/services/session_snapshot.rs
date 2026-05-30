use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};

use crate::app_state::{AppState, SessionFileChangeStats};
use crate::chat::{
    ChatState, CompletionCheckMessageState, Message, MessageRole, TodoDisplayStatus,
    TodoMessageState, ToolExecutionStatus, ToolMessageState,
};
use crate::gateway::{SessionLifecycleStatus, SessionRecord};
use crate::input::Input;

const SNAPSHOT_VERSION: u32 = 1;
const DEFAULT_SNAPSHOT_NAME: &str = "latest";

#[derive(Debug, Clone)]
pub struct SnapshotContext {
    pub name: String,
    pub parent_chain: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshotListEntry {
    pub name: String,
    pub snapshot_key: String,
    pub saved_at_ms: u64,
    pub parent_name: Option<String>,
    pub parent_chain: Vec<String>,
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshotDialog {
    pub entries: Vec<SessionSnapshotListEntry>,
    pub selected: usize,
}

impl SessionSnapshotDialog {
    pub fn new(entries: Vec<SessionSnapshotListEntry>) -> Self {
        Self {
            entries,
            selected: 0,
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

    pub fn selected_entry(&self) -> Option<&SessionSnapshotListEntry> {
        self.entries.get(self.selected)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiSessionSnapshot {
    pub version: u32,
    pub saved_at_ms: u64,
    #[serde(default)]
    pub parent_chain: Vec<String>,
    pub session_id: String,
    pub workspace: PathBuf,
    #[serde(default)]
    pub active_agent_role: Option<String>,
    #[serde(default)]
    pub reasoning_effort: agent_types::ReasoningEffort,
    #[serde(default)]
    pub session_messages: Vec<llm_client::ChatMessage>,
    #[serde(default)]
    pub plan_state: Option<SavedTodoMessageState>,
    #[serde(default)]
    pub chat_messages: Vec<SavedMessage>,
    #[serde(default)]
    pub session_file_changes: BTreeMap<String, SessionFileChangeStats>,
    #[serde(default)]
    pub session_record: Option<SessionRecord>,
    #[serde(default)]
    pub status_metrics: Option<SavedStatusMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedStatusMetrics {
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub input_context_tokens: u64,
    pub input_context_tokens_estimated: bool,
    pub last_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedMessage {
    pub role: SavedMessageRole,
    pub content: String,
    #[serde(default)]
    pub thinking_content: String,
    pub timestamp: String,
    #[serde(default)]
    pub tool_state: Option<SavedToolMessageState>,
    #[serde(default)]
    pub completion_check_state: Option<CompletionCheckMessageState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedMessageRole {
    User,
    Assistant,
    System,
    Error,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedToolMessageState {
    pub call_id: String,
    pub tool: String,
    pub summary: String,
    pub args_preview: String,
    #[serde(default)]
    pub command_preview: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    pub detail: String,
    pub expanded: bool,
    pub status: SavedToolExecutionStatus,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedToolExecutionStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTodoMessageState {
    pub title: String,
    pub items: Vec<(SavedTodoDisplayStatus, String)>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedTodoDisplayStatus {
    Pending,
    InProgress,
    Completed,
}

pub fn snapshot_name_from_command(trimmed: &str, command: &str) -> Result<String> {
    let first = trimmed.split_whitespace().next().unwrap_or("");
    if !first.eq_ignore_ascii_case(command) {
        bail!("expected {command}");
    }
    let rest = trimmed[first.len()..].trim();
    let name = if rest.is_empty() {
        DEFAULT_SNAPSHOT_NAME
    } else {
        rest
    };
    validate_snapshot_name(name)?;
    Ok(name.to_string())
}

pub fn snapshot_path(name: &str, parent_chain: Option<&[String]>) -> Result<PathBuf> {
    validate_snapshot_name(name)?;
    let dir = snapshot_dir()?;
    let filename = if let Some(chain) = parent_chain {
        if chain.is_empty() {
            format!("{name}.json")
        } else {
            let prefix = chain.join("_");
            format!("{prefix}_{name}.json")
        }
    } else {
        format!("{name}.json")
    };
    Ok(dir.join(filename))
}

pub fn build_snapshot(
    state: &AppState,
    session_record: Option<SessionRecord>,
    parent_chain: Vec<String>,
) -> TuiSessionSnapshot {
    let status_metrics = SavedStatusMetrics {
        total_tokens: state.status_panel.total_tokens,
        prompt_tokens: state.status_panel.prompt_tokens,
        completion_tokens: state.status_panel.completion_tokens,
        input_context_tokens: state.status_panel.input_context_tokens,
        input_context_tokens_estimated: state.status_panel.input_context_tokens_estimated,
        last_latency_ms: state.status_panel.last_latency_ms,
    };
    TuiSessionSnapshot {
        version: SNAPSHOT_VERSION,
        saved_at_ms: current_time_ms(),
        parent_chain,
        session_id: state.session_id.clone(),
        workspace: state.workspace.clone(),
        active_agent_role: state.active_agent_role.clone(),
        reasoning_effort: state.reasoning_effort,
        session_messages: state.session_messages.clone(),
        plan_state: state.plan_state.as_ref().map(SavedTodoMessageState::from),
        chat_messages: state
            .chat_state
            .messages
            .iter()
            .filter(|message| !message.is_streaming)
            .map(SavedMessage::from_message)
            .collect(),
        session_file_changes: state.session_file_changes.clone(),
        session_record,
        status_metrics: Some(status_metrics),
    }
}

pub fn save_snapshot_with_chain(
    name: &str,
    snapshot: &TuiSessionSnapshot,
    parent_chain: Option<&[String]>,
) -> Result<PathBuf> {
    let path = snapshot_path(name, parent_chain)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create snapshot directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(snapshot).context("failed to serialize snapshot")?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn load_snapshot_by_key(snapshot_key: &str) -> Result<(TuiSessionSnapshot, Vec<String>)> {
    let dir = snapshot_dir()?;
    let path = dir.join(format!("{snapshot_key}.json"));

    if !path.exists() {
        bail!("snapshot '{}' not found", snapshot_key);
    }

    parse_snapshot_file(&path, snapshot_key)
}

pub fn load_snapshot(name: &str) -> Result<Vec<(String, TuiSessionSnapshot, Vec<String>)>> {
    let dir = snapshot_dir()?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("snapshot '{}' not found", name)
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };

    let mut matching_snapshots = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(file_stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let snapshot_name = extract_snapshot_name(file_stem);
        if snapshot_name != name {
            continue;
        }

        if let Ok((snapshot, parent_chain)) = parse_snapshot_file(&path, file_stem) {
            matching_snapshots.push((file_stem.to_string(), snapshot, parent_chain));
        }
    }

    if matching_snapshots.is_empty() {
        bail!("snapshot '{}' not found", name)
    }

    matching_snapshots.sort_by(|a, b| {
        b.1.saved_at_ms.cmp(&a.1.saved_at_ms).then(a.0.cmp(&b.0))
    });

    Ok(matching_snapshots)
}

fn parse_snapshot_file(path: &Path, file_stem: &str) -> Result<(TuiSessionSnapshot, Vec<String>)> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot: TuiSessionSnapshot = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if snapshot.version != SNAPSHOT_VERSION {
        bail!(
            "unsupported snapshot version {} (expected {})",
            snapshot.version,
            SNAPSHOT_VERSION
        );
    }

    let parent_chain = if snapshot.parent_chain.is_empty() {
        extract_parent_chain(file_stem)
    } else {
        snapshot.parent_chain.clone()
    };

    Ok((snapshot, parent_chain))
}

fn extract_parent_chain(file_stem: &str) -> Vec<String> {
    let parts: Vec<&str> = file_stem.rsplit('_').collect();
    if parts.len() <= 1 {
        Vec::new()
    } else {
        parts[1..].iter().rev().map(|s| s.to_string()).collect()
    }
}

fn extract_snapshot_name(file_stem: &str) -> &str {
    file_stem.rsplit('_').next().unwrap_or(file_stem)
}

pub fn list_session_snapshots() -> Result<Vec<SessionSnapshotListEntry>> {
    let dir = snapshot_dir()?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };

    let mut snapshots = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(file_stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let name = extract_snapshot_name(file_stem).to_string();
        if validate_snapshot_name(&name).is_err() {
            continue;
        }
        let header = snapshot_header(&path);
        let saved_at_ms = header
            .as_ref()
            .and_then(|header| header.saved_at_ms)
            .or_else(|| file_timestamp_ms(&path));
        let parent_chain = header
            .as_ref()
            .map(|header| header.parent_chain.clone())
            .unwrap_or_else(|| extract_parent_chain(file_stem));
        let snapshot_key = file_stem.to_string();
        snapshots.push(SessionSnapshotListEntry {
            name,
            snapshot_key,
            saved_at_ms: saved_at_ms.unwrap_or(0),
            parent_name: parent_chain.last().cloned(),
            parent_chain,
            depth: 0,
        });
    }

    Ok(order_snapshots_by_parent(snapshots))
}

pub fn format_snapshot_time(saved_at_ms: u64) -> String {
    if saved_at_ms == 0 {
        return "unknown".to_string();
    }
    match Local.timestamp_millis_opt(saved_at_ms as i64).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "unknown".to_string(),
    }
}

pub fn apply_snapshot(
    state: &mut AppState,
    mut snapshot: TuiSessionSnapshot,
) -> Option<SessionRecord> {
    state.workspace = snapshot.workspace;
    state.status_panel.set_workspace(&state.workspace);
    state.session_id = snapshot.session_id;
    state.current_snapshot_context = None;
    state.active_agent_role = snapshot.active_agent_role;
    state.reasoning_effort = snapshot.reasoning_effort;
    state.session_messages = snapshot.session_messages;
    state.plan_state = snapshot.plan_state.map(Into::into);
    state.session_file_changes = snapshot.session_file_changes;
    state.tool_file_changes.clear();
    state.clear_tool_file_baselines();
    state.input_mode = crate::app_state::InputMode::Editing;
    state.provider_dialog = None;
    state.api_key_dialog = None;
    state.interaction_prompt = None;
    state.transcript_selection = None;
    state.copy_notice = None;
    state.slash = Default::default();
    state.render_state = Default::default();
    state.external_commands = crate::services::command_loader::load_external_commands();
    state.chat_state = chat_state_with_messages(&state.agent_config, snapshot.chat_messages);

    if let Some(metrics) = snapshot.status_metrics {
        state.status_panel.total_tokens = metrics.total_tokens;
        state.status_panel.prompt_tokens = metrics.prompt_tokens;
        state.status_panel.completion_tokens = metrics.completion_tokens;
        state.status_panel.input_context_tokens = metrics.input_context_tokens;
        state.status_panel.input_context_tokens_estimated = metrics.input_context_tokens_estimated;
        state.status_panel.last_latency_ms = metrics.last_latency_ms;
    }

    snapshot.session_record.as_mut().map(|record| {
        record.status = SessionLifecycleStatus::Idle;
        record.last_error = None;
        record.clone()
    })
}

fn chat_state_with_messages(
    config: &crate::config::Config,
    saved_messages: Vec<SavedMessage>,
) -> ChatState {
    let mut chat_state = crate::app_state::build_chat_state(config);
    chat_state.messages = saved_messages
        .into_iter()
        .map(SavedMessage::into_message)
        .collect();
    chat_state.input = Input::default();
    chat_state.is_loading = false;
    chat_state.stick_to_bottom = true;
    chat_state
}

fn validate_snapshot_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("snapshot name must contain only letters, numbers, '-', '_' or '.'");
    }
    Ok(())
}

fn snapshot_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("unable to resolve home directory for ~/.xiaoo/session")?;
    Ok(home.join(".xiaoo").join("session"))
}

#[derive(Deserialize)]
struct SnapshotHeader {
    saved_at_ms: Option<u64>,
    #[serde(default)]
    parent_chain: Vec<String>,
}

fn snapshot_header(path: &Path) -> Option<SnapshotHeader> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str::<SnapshotHeader>(&content).ok()
}

fn file_timestamp_ms(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let time = metadata.created().or_else(|_| metadata.modified()).ok()?;
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn order_snapshots_by_parent(
    entries: Vec<SessionSnapshotListEntry>,
) -> Vec<SessionSnapshotListEntry> {
    let mut by_parent: HashMap<Vec<String>, Vec<SessionSnapshotListEntry>> = HashMap::new();

    for entry in entries {
        let parent_key = entry.parent_chain.clone();
        by_parent.entry(parent_key).or_default().push(entry);
    }

    for children in by_parent.values_mut() {
        children.sort_by(|left, right| {
            right
                .saved_at_ms
                .cmp(&left.saved_at_ms)
                .then(left.name.cmp(&right.name))
        });
    }

    let mut ordered = Vec::new();
    append_snapshot_children_by_chain(Vec::new(), 0, &mut by_parent, &mut ordered);
    while let Some(parent_key) = by_parent.keys().next().cloned() {
        append_snapshot_children_by_chain(parent_key, 0, &mut by_parent, &mut ordered);
    }
    ordered
}

fn append_snapshot_children_by_chain(
    parent_key: Vec<String>,
    depth: usize,
    by_parent: &mut HashMap<Vec<String>, Vec<SessionSnapshotListEntry>>,
    ordered: &mut Vec<SessionSnapshotListEntry>,
) {
    let Some(children) = by_parent.remove(&parent_key) else {
        return;
    };
    for mut child in children {
        let mut child_key = child.parent_chain.clone();
        child_key.push(child.name.clone());
        child.depth = depth;
        ordered.push(child);
        append_snapshot_children_by_chain(child_key, depth + 1, by_parent, ordered);
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

impl SavedMessage {
    fn from_message(message: &Message) -> Self {
        Self {
            role: SavedMessageRole::from(message.role),
            content: message.content.clone(),
            thinking_content: message.thinking_content.clone(),
            timestamp: message.timestamp.to_rfc3339(),
            tool_state: message.tool_state.as_ref().map(SavedToolMessageState::from),
            completion_check_state: message.completion_check_state.clone(),
        }
    }

    fn into_message(self) -> Message {
        Message {
            role: self.role.into(),
            content: self.content,
            thinking_content: self.thinking_content,
            timestamp: self
                .timestamp
                .parse::<DateTime<chrono::FixedOffset>>()
                .map(|dt| dt.with_timezone(&Local))
                .unwrap_or_else(|_| Local::now()),
            is_streaming: false,
            tool_state: self.tool_state.map(Into::into),
            completion_check_state: self.completion_check_state,
            render_revision: 0,
        }
    }
}

impl From<MessageRole> for SavedMessageRole {
    fn from(role: MessageRole) -> Self {
        match role {
            MessageRole::User => Self::User,
            MessageRole::Assistant => Self::Assistant,
            MessageRole::System => Self::System,
            MessageRole::Error => Self::Error,
            MessageRole::Tool => Self::Tool,
        }
    }
}

impl From<SavedMessageRole> for MessageRole {
    fn from(role: SavedMessageRole) -> Self {
        match role {
            SavedMessageRole::User => Self::User,
            SavedMessageRole::Assistant => Self::Assistant,
            SavedMessageRole::System => Self::System,
            SavedMessageRole::Error => Self::Error,
            SavedMessageRole::Tool => Self::Tool,
        }
    }
}

impl From<&ToolMessageState> for SavedToolMessageState {
    fn from(state: &ToolMessageState) -> Self {
        Self {
            call_id: state.call_id.clone(),
            tool: state.tool.clone(),
            summary: state.summary.clone(),
            args_preview: state.args_preview.clone(),
            command_preview: state.command_preview.clone(),
            command: state.command.clone(),
            detail: state.detail.clone(),
            expanded: state.expanded,
            status: state.status.into(),
            exit_code: state.exit_code,
            duration_ms: state.duration_ms,
        }
    }
}

impl From<SavedToolMessageState> for ToolMessageState {
    fn from(state: SavedToolMessageState) -> Self {
        Self {
            call_id: state.call_id,
            tool: state.tool,
            summary: state.summary,
            args_preview: state.args_preview,
            command_preview: state.command_preview,
            command: state.command,
            detail: state.detail,
            expanded: state.expanded,
            status: state.status.into(),
            exit_code: state.exit_code,
            duration_ms: state.duration_ms,
        }
    }
}

impl From<ToolExecutionStatus> for SavedToolExecutionStatus {
    fn from(status: ToolExecutionStatus) -> Self {
        match status {
            ToolExecutionStatus::Running => Self::Running,
            ToolExecutionStatus::Completed => Self::Completed,
            ToolExecutionStatus::Failed => Self::Failed,
        }
    }
}

impl From<SavedToolExecutionStatus> for ToolExecutionStatus {
    fn from(status: SavedToolExecutionStatus) -> Self {
        match status {
            SavedToolExecutionStatus::Running => Self::Running,
            SavedToolExecutionStatus::Completed => Self::Completed,
            SavedToolExecutionStatus::Failed => Self::Failed,
        }
    }
}

impl From<&TodoMessageState> for SavedTodoMessageState {
    fn from(state: &TodoMessageState) -> Self {
        Self {
            title: state.title.clone(),
            items: state
                .items
                .iter()
                .map(|(status, content)| ((*status).into(), content.clone()))
                .collect(),
        }
    }
}

impl From<SavedTodoMessageState> for TodoMessageState {
    fn from(state: SavedTodoMessageState) -> Self {
        Self {
            title: state.title,
            items: state
                .items
                .into_iter()
                .map(|(status, content)| (status.into(), content))
                .collect(),
        }
    }
}

impl From<TodoDisplayStatus> for SavedTodoDisplayStatus {
    fn from(status: TodoDisplayStatus) -> Self {
        match status {
            TodoDisplayStatus::Pending => Self::Pending,
            TodoDisplayStatus::InProgress => Self::InProgress,
            TodoDisplayStatus::Completed => Self::Completed,
        }
    }
}

impl From<SavedTodoDisplayStatus> for TodoDisplayStatus {
    fn from(status: SavedTodoDisplayStatus) -> Self {
        match status {
            SavedTodoDisplayStatus::Pending => Self::Pending,
            SavedTodoDisplayStatus::InProgress => Self::InProgress,
            SavedTodoDisplayStatus::Completed => Self::Completed,
        }
    }
}

#[allow(dead_code)]
fn _snapshot_dir_for_tests(path: &Path) -> PathBuf {
    path.join(".xiaoo").join("session")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_path_without_parent() {
        let path = snapshot_path("test", None).unwrap();
        assert_eq!(path.file_name().unwrap(), "test.json");
    }

    #[test]
    fn test_snapshot_path_with_parent_chain() {
        let chain = vec!["parent".to_string()];
        let path = snapshot_path("child", Some(&chain)).unwrap();
        assert_eq!(path.file_name().unwrap(), "parent_child.json");

        let chain = vec!["grandparent".to_string(), "parent".to_string()];
        let path = snapshot_path("child", Some(&chain)).unwrap();
        assert_eq!(path.file_name().unwrap(), "grandparent_parent_child.json");
    }

    #[test]
    fn test_extract_snapshot_name() {
        assert_eq!(extract_snapshot_name("test"), "test");
        assert_eq!(extract_snapshot_name("parent_test"), "test");
        assert_eq!(extract_snapshot_name("grandparent_parent_test"), "test");
        assert_eq!(extract_snapshot_name("a_b_c_d"), "d");
    }

    #[test]
    fn test_validate_snapshot_name() {
        assert!(validate_snapshot_name("test123").is_ok());
        assert!(validate_snapshot_name("test-123").is_ok());
        assert!(validate_snapshot_name("test_123").is_ok());
        assert!(validate_snapshot_name("test.123").is_ok());
        assert!(validate_snapshot_name("").is_err());
        assert!(validate_snapshot_name(".").is_err());
        assert!(validate_snapshot_name("..").is_err());
        assert!(validate_snapshot_name("test 123").is_err());
        assert!(validate_snapshot_name("test/123").is_err());
    }

    #[test]
    fn test_extract_parent_chain() {
        assert_eq!(extract_parent_chain("test"), Vec::<String>::new());
        assert_eq!(extract_parent_chain("parent_test"), vec!["parent"]);
        assert_eq!(
            extract_parent_chain("grandparent_parent_test"),
            vec!["grandparent", "parent"]
        );
        assert_eq!(extract_parent_chain("a_b_c_d"), vec!["a", "b", "c"]);
    }
}
