use std::collections::BTreeMap;
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
pub struct SessionSnapshotListEntry {
    pub name: String,
    pub saved_at_ms: u64,
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
    pub session_id: String,
    pub workspace: PathBuf,
    #[serde(default)]
    pub active_agent_role: Option<String>,
    #[serde(default)]
    pub session_messages: Vec<llm_client::ChatMessage>,
    #[serde(default)]
    pub chat_messages: Vec<SavedMessage>,
    #[serde(default)]
    pub session_file_changes: BTreeMap<String, SessionFileChangeStats>,
    #[serde(default)]
    pub session_record: Option<SessionRecord>,
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
    pub todo_state: Option<SavedTodoMessageState>,
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

pub fn snapshot_path(name: &str) -> Result<PathBuf> {
    validate_snapshot_name(name)?;
    Ok(snapshot_dir()?.join(format!("{name}.json")))
}

pub fn build_snapshot(
    state: &AppState,
    session_record: Option<SessionRecord>,
) -> TuiSessionSnapshot {
    TuiSessionSnapshot {
        version: SNAPSHOT_VERSION,
        saved_at_ms: current_time_ms(),
        session_id: state.session_id.clone(),
        workspace: state.workspace.clone(),
        active_agent_role: state.active_agent_role.clone(),
        session_messages: state.session_messages.clone(),
        chat_messages: state
            .chat_state
            .messages
            .iter()
            .filter(|message| !message.is_streaming)
            .map(SavedMessage::from_message)
            .collect(),
        session_file_changes: state.session_file_changes.clone(),
        session_record,
    }
}

pub fn save_snapshot(name: &str, snapshot: &TuiSessionSnapshot) -> Result<PathBuf> {
    let path = snapshot_path(name)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create snapshot directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(snapshot).context("failed to serialize snapshot")?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn load_snapshot(name: &str) -> Result<TuiSessionSnapshot> {
    let path = snapshot_path(name)?;
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot: TuiSessionSnapshot = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if snapshot.version != SNAPSHOT_VERSION {
        bail!(
            "unsupported snapshot version {} (expected {})",
            snapshot.version,
            SNAPSHOT_VERSION
        );
    }
    Ok(snapshot)
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
        let Some(name) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        if validate_snapshot_name(&name).is_err() {
            continue;
        }
        let saved_at_ms = snapshot_saved_at_ms(&path).or_else(|| file_timestamp_ms(&path));
        snapshots.push(SessionSnapshotListEntry {
            name,
            saved_at_ms: saved_at_ms.unwrap_or(0),
        });
    }

    snapshots.sort_by(|left, right| {
        right
            .saved_at_ms
            .cmp(&left.saved_at_ms)
            .then(left.name.cmp(&right.name))
    });
    Ok(snapshots)
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
    state.active_agent_role = snapshot.active_agent_role;
    state.session_messages = snapshot.session_messages;
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
}

fn snapshot_saved_at_ms(path: &Path) -> Option<u64> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str::<SnapshotHeader>(&content)
        .ok()?
        .saved_at_ms
}

fn file_timestamp_ms(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let time = metadata.created().or_else(|_| metadata.modified()).ok()?;
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
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
            todo_state: message.todo_state.as_ref().map(SavedTodoMessageState::from),
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
            todo_state: self.todo_state.map(Into::into),
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
