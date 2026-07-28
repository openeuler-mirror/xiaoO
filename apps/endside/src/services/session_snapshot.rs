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

const SNAPSHOT_VERSION: u32 = 2;
const AUTO_SNAPSHOT_KEY_PREFIX: &str = "@auto-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    Manual,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualSnapshotRef {
    pub snapshot_id: String,
    pub snapshot_key: String,
    pub name: String,
    #[serde(default)]
    pub parent_chain: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SnapshotContext {
    pub kind: SnapshotKind,
    pub snapshot_id: String,
    pub snapshot_key: String,
    pub name: String,
    pub parent_chain: Vec<String>,
    pub base_manual: Option<ManualSnapshotRef>,
}

impl SnapshotContext {
    pub fn from_snapshot(snapshot_key: String, snapshot: &TuiSessionSnapshot) -> Self {
        Self {
            kind: snapshot.kind,
            snapshot_id: snapshot.snapshot_id.clone(),
            snapshot_key,
            name: snapshot.name.clone(),
            parent_chain: snapshot.parent_chain.clone(),
            base_manual: snapshot.base_manual.clone(),
        }
    }

    fn manual_ref(&self) -> Option<ManualSnapshotRef> {
        match self.kind {
            SnapshotKind::Manual => Some(ManualSnapshotRef {
                snapshot_id: self.snapshot_id.clone(),
                snapshot_key: self.snapshot_key.clone(),
                name: self.name.clone(),
                parent_chain: self.parent_chain.clone(),
            }),
            SnapshotKind::Auto => self.base_manual.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSnapshotListEntry {
    pub kind: SnapshotKind,
    pub name: String,
    pub snapshot_key: String,
    pub saved_at_ms: u64,
    pub parent_name: Option<String>,
    pub parent_chain: Vec<String>,
    pub depth: usize,
    pub base_manual_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionSnapshotCatalog {
    pub manual: Vec<SessionSnapshotListEntry>,
    pub automatic: Vec<SessionSnapshotListEntry>,
}

impl SessionSnapshotCatalog {
    pub fn is_empty(&self) -> bool {
        self.manual.is_empty() && self.automatic.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSnapshotPane {
    Manual,
    Automatic,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshotDialog {
    pub manual_entries: Vec<SessionSnapshotListEntry>,
    pub automatic_entries: Vec<SessionSnapshotListEntry>,
    pub active_pane: SessionSnapshotPane,
    pub manual_selected: usize,
    pub automatic_selected: usize,
}

impl SessionSnapshotDialog {
    pub fn new(catalog: SessionSnapshotCatalog) -> Self {
        let active_pane = if catalog.manual.is_empty() {
            SessionSnapshotPane::Automatic
        } else {
            SessionSnapshotPane::Manual
        };
        Self {
            manual_entries: catalog.manual,
            automatic_entries: catalog.automatic,
            active_pane,
            manual_selected: 0,
            automatic_selected: 0,
        }
    }

    pub fn manual_only(entries: Vec<SessionSnapshotListEntry>) -> Self {
        Self::new(SessionSnapshotCatalog {
            manual: entries,
            automatic: Vec::new(),
        })
    }

    pub fn move_up(&mut self) {
        match self.active_pane {
            SessionSnapshotPane::Manual => {
                self.manual_selected = self.manual_selected.saturating_sub(1)
            }
            SessionSnapshotPane::Automatic => {
                self.automatic_selected = self.automatic_selected.saturating_sub(1)
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.active_pane {
            SessionSnapshotPane::Manual if !self.manual_entries.is_empty() => {
                self.manual_selected =
                    (self.manual_selected + 1).min(self.manual_entries.len() - 1);
            }
            SessionSnapshotPane::Automatic if !self.automatic_entries.is_empty() => {
                self.automatic_selected =
                    (self.automatic_selected + 1).min(self.automatic_entries.len() - 1);
            }
            _ => {}
        }
    }

    pub fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            SessionSnapshotPane::Manual if !self.automatic_entries.is_empty() => {
                SessionSnapshotPane::Automatic
            }
            SessionSnapshotPane::Automatic if !self.manual_entries.is_empty() => {
                SessionSnapshotPane::Manual
            }
            current => current,
        };
    }

    pub fn selected_entry(&self) -> Option<&SessionSnapshotListEntry> {
        match self.active_pane {
            SessionSnapshotPane::Manual => self.manual_entries.get(self.manual_selected),
            SessionSnapshotPane::Automatic => self.automatic_entries.get(self.automatic_selected),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiSessionSnapshot {
    pub version: u32,
    pub kind: SnapshotKind,
    pub snapshot_id: String,
    pub name: String,
    pub saved_at_ms: u64,
    #[serde(default)]
    pub parent_chain: Vec<String>,
    #[serde(default)]
    pub base_manual: Option<ManualSnapshotRef>,
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
    if rest.is_empty() {
        bail!("snapshot name is required");
    }
    validate_snapshot_name(rest)?;
    Ok(rest.to_string())
}

pub fn manual_snapshot_name_from_command(trimmed: &str, command: &str) -> Result<Option<String>> {
    let first = trimmed.split_whitespace().next().unwrap_or("");
    if !first.eq_ignore_ascii_case(command) {
        bail!("expected {command}");
    }
    let rest = trimmed[first.len()..].trim();
    if rest.is_empty() {
        return Ok(None);
    }
    validate_snapshot_name(rest)?;
    Ok(Some(rest.to_string()))
}

fn manual_snapshot_path_in_dir(dir: &Path, name: &str, parent_chain: &[String]) -> Result<PathBuf> {
    validate_snapshot_name(name)?;
    let filename = if parent_chain.is_empty() {
        format!("{name}.json")
    } else {
        let prefix = parent_chain.join("_");
        format!("{prefix}_{name}.json")
    };
    Ok(dir.join(filename))
}

struct SnapshotDescriptor {
    kind: SnapshotKind,
    snapshot_id: String,
    name: String,
    parent_chain: Vec<String>,
    base_manual: Option<ManualSnapshotRef>,
}

fn build_snapshot(
    state: &AppState,
    session_record: Option<SessionRecord>,
    descriptor: SnapshotDescriptor,
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
        kind: descriptor.kind,
        snapshot_id: descriptor.snapshot_id,
        name: descriptor.name,
        saved_at_ms: current_time_ms(),
        parent_chain: descriptor.parent_chain,
        base_manual: descriptor.base_manual,
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
        session_file_changes: state.session_file_changes().clone(),
        session_record,
        status_metrics: Some(status_metrics),
    }
}

fn save_snapshot_at_path(path: &Path, snapshot: &TuiSessionSnapshot) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create snapshot directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(snapshot).context("failed to serialize snapshot")?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path.to_path_buf())
}

pub fn save_manual_snapshot(
    state: &AppState,
    session_record: Option<SessionRecord>,
    requested_name: Option<&str>,
) -> Result<(PathBuf, SnapshotContext)> {
    save_manual_snapshot_in_dir(&snapshot_dir()?, state, session_record, requested_name)
}

fn save_manual_snapshot_in_dir(
    dir: &Path,
    state: &AppState,
    session_record: Option<SessionRecord>,
    requested_name: Option<&str>,
) -> Result<(PathBuf, SnapshotContext)> {
    let anchor = state
        .current_snapshot_context
        .as_ref()
        .and_then(SnapshotContext::manual_ref);
    let explicit_name = requested_name
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = explicit_name {
        validate_snapshot_name(name)?;
    }

    let name = match explicit_name {
        Some(name) => name.to_string(),
        None => unique_generated_manual_name(dir, state)?,
    };

    let (path, parent_chain) = match anchor.as_ref() {
        Some(anchor) if explicit_name.is_some() && anchor.name == name => (
            path_for_snapshot_key(dir, &anchor.snapshot_key)?,
            anchor.parent_chain.clone(),
        ),
        Some(anchor) => {
            let mut chain = anchor.parent_chain.clone();
            chain.push(anchor.name.clone());
            (manual_snapshot_path_in_dir(dir, &name, &chain)?, chain)
        }
        None => (manual_snapshot_path_in_dir(dir, &name, &[])?, Vec::new()),
    };

    let existing = if path.exists() {
        Some(
            parse_snapshot_file(&path, snapshot_key_from_path(&path)?).with_context(|| {
                format!(
                "snapshot target {} is an unsupported legacy or corrupt file; choose another name",
                path.display()
            )
            })?,
        )
    } else {
        None
    };
    if existing
        .as_ref()
        .is_some_and(|snapshot| snapshot.kind == SnapshotKind::Auto)
    {
        bail!("manual snapshot target is reserved for automatic saves");
    }
    let snapshot_id = existing
        .filter(|snapshot| snapshot.kind == SnapshotKind::Manual)
        .map(|snapshot| snapshot.snapshot_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let snapshot = build_snapshot(
        state,
        session_record,
        SnapshotDescriptor {
            kind: SnapshotKind::Manual,
            snapshot_id,
            name,
            parent_chain,
            base_manual: None,
        },
    );
    let path = save_snapshot_at_path(&path, &snapshot)?;
    let snapshot_key = snapshot_key_from_path(&path)?.to_string();
    let context = SnapshotContext::from_snapshot(snapshot_key, &snapshot);
    Ok((path, context))
}

/// Auto-save the current session when the user interrupts the runtime
/// (Ctrl+C / SIGINT / SIGTERM).
///
/// Manual snapshots are immutable from this path. A loaded manual snapshot
/// owns one rolling auto-save slot, while a loaded auto-save updates itself.
/// Sessions without a manual anchor create one unbound auto-save.
///
/// Returns `Ok(None)` when there is no user prompt to summarise (nothing worth
/// saving) and the session is not already associated with a snapshot.
pub fn autosave_on_interrupt(
    state: &AppState,
    session_record: Option<SessionRecord>,
) -> Result<Option<PathBuf>> {
    autosave_on_interrupt_in_dir(&snapshot_dir()?, state, session_record)
}

fn autosave_on_interrupt_in_dir(
    dir: &Path,
    state: &AppState,
    session_record: Option<SessionRecord>,
) -> Result<Option<PathBuf>> {
    let context = state.current_snapshot_context.as_ref();
    let (path, snapshot_id, name, base_manual) = match context {
        Some(context) if context.kind == SnapshotKind::Auto => (
            path_for_snapshot_key(dir, &context.snapshot_key)?,
            context.snapshot_id.clone(),
            context.name.clone(),
            context.base_manual.clone(),
        ),
        Some(context) => {
            let base_manual = context
                .manual_ref()
                .context("manual snapshot context is missing its manual identity")?;
            let path = dir.join(format!(
                "{AUTO_SNAPSHOT_KEY_PREFIX}{}.json",
                base_manual.snapshot_id
            ));
            if !path.exists() {
                (
                    path,
                    uuid::Uuid::new_v4().to_string(),
                    generated_snapshot_name(state),
                    Some(base_manual),
                )
            } else {
                match parse_snapshot_file(&path, snapshot_key_from_path(&path)?) {
                    Ok(existing) if existing.kind == SnapshotKind::Auto => {
                        (path, existing.snapshot_id, existing.name, Some(base_manual))
                    }
                    Ok(_) => bail!("automatic snapshot path contains a manual snapshot"),
                    Err(error) => return Err(error).context(
                        "automatic snapshot path contains an unsupported legacy or corrupt file",
                    ),
                }
            }
        }
        None => {
            if autosave_topic(state).is_none() {
                return Ok(None);
            }
            let snapshot_id = uuid::Uuid::new_v4().to_string();
            (
                dir.join(format!("{AUTO_SNAPSHOT_KEY_PREFIX}{snapshot_id}.json")),
                snapshot_id,
                generated_snapshot_name(state),
                None,
            )
        }
    };
    let snapshot = build_snapshot(
        state,
        session_record,
        SnapshotDescriptor {
            kind: SnapshotKind::Auto,
            snapshot_id,
            name,
            parent_chain: Vec::new(),
            base_manual,
        },
    );
    save_snapshot_at_path(&path, &snapshot).map(Some)
}

/// Derive a short topic label (≤10 characters) from the first user prompt.
/// Returns `None` when the session has no user messages.
fn autosave_topic(state: &AppState) -> Option<String> {
    let first =
        state.chat_state.messages.iter().find(|message| {
            message.role == MessageRole::User && !message.content.trim().is_empty()
        })?;
    Some(sanitize_topic(first.content.trim()))
}

/// Flatten whitespace, cap at 10 characters, then make the result a valid
/// snapshot name: replace every non-alphanumeric rune with `-`, collapse
/// consecutive dashes and trim trailing ones. Falls back to `"untitled"` when
/// nothing usable remains (e.g. a prompt made solely of punctuation).
fn sanitize_topic(text: &str) -> String {
    let flattened: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = flattened.chars().take(10).collect();
    let mut out = String::with_capacity(truncated.len());
    let mut prev_dash = false;
    for ch in truncated.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

fn generated_snapshot_name(state: &AppState) -> String {
    let date = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let topic = autosave_topic(state).unwrap_or_else(|| "untitled".to_string());
    format!("{date}-{topic}")
}

fn unique_generated_manual_name(dir: &Path, state: &AppState) -> Result<String> {
    let base = generated_snapshot_name(state);
    let existing_names = list_session_snapshots_in_dir(dir)?
        .manual
        .into_iter()
        .map(|entry| entry.name)
        .collect::<std::collections::HashSet<_>>();
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while existing_names.contains(&candidate) || dir.join(format!("{candidate}.json")).exists() {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    Ok(candidate)
}

pub fn load_snapshot_by_key(snapshot_key: &str) -> Result<(TuiSessionSnapshot, Vec<String>)> {
    load_snapshot_by_key_in_dir(&snapshot_dir()?, snapshot_key)
}

fn load_snapshot_by_key_in_dir(
    dir: &Path,
    snapshot_key: &str,
) -> Result<(TuiSessionSnapshot, Vec<String>)> {
    let path = path_for_snapshot_key(dir, snapshot_key)?;
    if !path.exists() {
        bail!("snapshot '{}' not found", snapshot_key);
    }
    let snapshot = parse_snapshot_file(&path, snapshot_key)?;
    let parent_chain = snapshot.parent_chain.clone();
    Ok((snapshot, parent_chain))
}

pub fn load_snapshot(name: &str) -> Result<Vec<(String, TuiSessionSnapshot, Vec<String>)>> {
    load_snapshot_in_dir(&snapshot_dir()?, name)
}

fn load_snapshot_in_dir(
    dir: &Path,
    name: &str,
) -> Result<Vec<(String, TuiSessionSnapshot, Vec<String>)>> {
    validate_snapshot_name(name)?;
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
        if let Ok(snapshot) = parse_snapshot_file(&path, file_stem) {
            if snapshot.kind == SnapshotKind::Manual && snapshot.name == name {
                let parent_chain = snapshot.parent_chain.clone();
                matching_snapshots.push((file_stem.to_string(), snapshot, parent_chain));
            }
        }
    }

    if matching_snapshots.is_empty() {
        bail!("snapshot '{}' not found", name)
    }

    matching_snapshots.sort_by(|a, b| b.1.saved_at_ms.cmp(&a.1.saved_at_ms).then(a.0.cmp(&b.0)));

    Ok(matching_snapshots)
}

fn parse_snapshot_file(path: &Path, _file_stem: &str) -> Result<TuiSessionSnapshot> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot: TuiSessionSnapshot = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if snapshot.version != SNAPSHOT_VERSION {
        bail!(
            "unsupported snapshot version {} (expected {})",
            snapshot.version,
            SNAPSHOT_VERSION
        );
    }
    uuid::Uuid::parse_str(&snapshot.snapshot_id)
        .with_context(|| format!("invalid snapshot id in {}", path.display()))?;
    validate_snapshot_name(&snapshot.name)
        .with_context(|| format!("invalid snapshot name in {}", path.display()))?;
    for parent in &snapshot.parent_chain {
        validate_snapshot_name(parent)
            .with_context(|| format!("invalid parent snapshot name in {}", path.display()))?;
    }
    if snapshot.kind == SnapshotKind::Manual && snapshot.base_manual.is_some() {
        bail!("manual snapshot cannot reference a base manual snapshot");
    }
    if let Some(base) = snapshot.base_manual.as_ref() {
        uuid::Uuid::parse_str(&base.snapshot_id)
            .with_context(|| format!("invalid base manual id in {}", path.display()))?;
        path_for_snapshot_key(Path::new("."), &base.snapshot_key)
            .with_context(|| format!("invalid base manual key in {}", path.display()))?;
        validate_snapshot_name(&base.name)
            .with_context(|| format!("invalid base manual name in {}", path.display()))?;
        for parent in &base.parent_chain {
            validate_snapshot_name(parent)
                .with_context(|| format!("invalid base parent name in {}", path.display()))?;
        }
    }
    Ok(snapshot)
}

pub fn list_session_snapshots() -> Result<SessionSnapshotCatalog> {
    list_session_snapshots_in_dir(&snapshot_dir()?)
}

fn list_session_snapshots_in_dir(dir: &Path) -> Result<SessionSnapshotCatalog> {
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionSnapshotCatalog::default())
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };

    let mut manual = Vec::new();
    let mut automatic = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(file_stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(snapshot) = parse_snapshot_file(&path, file_stem) else {
            continue;
        };
        let snapshot_key = file_stem.to_string();
        let mut list_entry = SessionSnapshotListEntry {
            kind: snapshot.kind,
            name: snapshot.name,
            snapshot_key,
            saved_at_ms: snapshot.saved_at_ms,
            parent_name: snapshot.parent_chain.last().cloned(),
            parent_chain: snapshot.parent_chain,
            depth: 0,
            base_manual_name: snapshot.base_manual.map(|base| base.name),
        };
        match list_entry.kind {
            SnapshotKind::Manual => manual.push(list_entry),
            SnapshotKind::Auto => {
                list_entry.parent_name = None;
                automatic.push(list_entry);
            }
        }
    }

    automatic.sort_by(|left, right| {
        right
            .saved_at_ms
            .cmp(&left.saved_at_ms)
            .then(left.name.cmp(&right.name))
    });
    Ok(SessionSnapshotCatalog {
        manual: order_snapshots_by_parent(manual),
        automatic,
    })
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
    state.sync_diff_tracker_workspace();
    state.session_id = snapshot.session_id;
    state.current_snapshot_context = None;
    state.active_agent_role = snapshot.active_agent_role;
    state.reasoning_effort = snapshot.reasoning_effort;
    state.session_messages = snapshot.session_messages;
    state.plan_state = snapshot.plan_state.map(Into::into);
    state.restore_session_file_changes(snapshot.session_file_changes);
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
    let input_history = state.chat_state.input_history.clone();
    state.chat_state = chat_state_with_messages(&state.agent_config, snapshot.chat_messages);
    state.chat_state.set_input_history(input_history);

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
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("snapshot name must contain only letters, numbers, '-', '_' or '.'");
    }
    Ok(())
}

fn path_for_snapshot_key(dir: &Path, snapshot_key: &str) -> Result<PathBuf> {
    if snapshot_key.is_empty()
        || snapshot_key == "."
        || snapshot_key == ".."
        || snapshot_key.contains('/')
        || snapshot_key.contains('\\')
    {
        bail!("invalid snapshot key");
    }
    Ok(dir.join(format!("{snapshot_key}.json")))
}

fn snapshot_key_from_path(path: &Path) -> Result<&str> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .context("snapshot path has no valid file name")
}

fn snapshot_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("unable to resolve home directory for ~/.xiaoo/session")?;
    Ok(home.join(".xiaoo").join("session"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_prompt(prompt: &str) -> AppState {
        let mut state = AppState::new(PathBuf::new(), PathBuf::new()).unwrap();
        state.chat_state.messages.push(Message::user(prompt));
        state
    }

    fn read_snapshot(path: &Path) -> TuiSessionSnapshot {
        let key = snapshot_key_from_path(path).unwrap();
        parse_snapshot_file(path, key).unwrap()
    }

    fn json_file_count(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .count()
    }

    #[test]
    fn test_sanitize_topic_caps_at_ten_chars() {
        // Whitespace is flattened then the first 10 chars are kept.
        assert_eq!(sanitize_topic("hello world"), "hello-worl");
        // Long ASCII prompts are truncated to 10 chars.
        assert_eq!(
            sanitize_topic("the quick brown fox jumps over the lazy dog"),
            "the-quick"
        );
    }

    #[test]
    fn test_sanitize_topic_keeps_unicode_and_drops_punctuation() {
        // Chinese letters are alphanumeric and therefore preserved; the space
        // between words becomes a single dash.
        assert_eq!(
            sanitize_topic("帮我为xiaoo agent runtime增加会话自动保存机制"),
            "帮我为xiaoo-a"
        );
        // A prompt made solely of punctuation yields the fallback label.
        assert_eq!(sanitize_topic("!@#$%^&*()"), "untitled");
        // Whitespace-only input also falls back.
        assert_eq!(sanitize_topic("    "), "untitled");
    }

    #[test]
    fn test_autosave_topic_uses_first_user_prompt() {
        let mut state = AppState::new(PathBuf::new(), PathBuf::new()).unwrap();
        // No user messages → nothing to summarise.
        assert_eq!(autosave_topic(&state), None);

        state
            .chat_state
            .messages
            .push(Message::user("帮我为xiaoo agent runtime增加保存机制"));
        assert_eq!(autosave_topic(&state), Some("帮我为xiaoo-a".to_string()));

        // A subsequent user prompt must not override the first one.
        state
            .chat_state
            .messages
            .push(Message::user("another unrelated question"));
        assert_eq!(autosave_topic(&state), Some("帮我为xiaoo-a".to_string()));
    }

    #[test]
    fn manual_path_builds_parent_chain() {
        let dir = Path::new("/tmp/xiaoo-session-test");
        assert_eq!(
            manual_snapshot_path_in_dir(dir, "child", &["parent".to_string()])
                .unwrap()
                .file_name()
                .unwrap(),
            "parent_child.json"
        );
    }

    #[test]
    fn validate_snapshot_name_reserves_auto_namespace() {
        assert!(validate_snapshot_name("test123").is_ok());
        assert!(validate_snapshot_name("test-123").is_ok());
        assert!(validate_snapshot_name("test_123").is_ok());
        assert!(validate_snapshot_name("test.123").is_ok());
        assert!(validate_snapshot_name("@auto-id").is_err());
        assert!(validate_snapshot_name("").is_err());
        assert!(validate_snapshot_name(".").is_err());
        assert!(validate_snapshot_name("..").is_err());
        assert!(validate_snapshot_name("test 123").is_err());
        assert!(validate_snapshot_name("test/123").is_err());
    }

    #[test]
    fn autosave_from_manual_never_rewrites_manual_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("protect this checkpoint");
        let (manual_path, manual_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("checkpoint")).unwrap();
        let manual_before = fs::read(&manual_path).unwrap();
        let manual_id = manual_context.snapshot_id.clone();
        state.current_snapshot_context = Some(manual_context);
        state
            .chat_state
            .messages
            .push(Message::system("later work"));

        let auto_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();

        assert_ne!(manual_path, auto_path);
        assert_eq!(fs::read(&manual_path).unwrap(), manual_before);
        let auto = read_snapshot(&auto_path);
        assert_eq!(auto.kind, SnapshotKind::Auto);
        assert_eq!(
            auto.base_manual
                .as_ref()
                .map(|base| base.snapshot_id.as_str()),
            Some(manual_id.as_str())
        );
    }

    #[test]
    fn autosave_rolls_forward_one_slot_per_manual_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("rolling auto save");
        let (_, first_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("first")).unwrap();
        state.current_snapshot_context = Some(first_context);
        let first_auto = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();
        let first_auto_id = read_snapshot(&first_auto).snapshot_id;

        state
            .chat_state
            .messages
            .push(Message::system("newer content"));
        let repeated_auto = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();
        assert_eq!(first_auto, repeated_auto);
        assert_eq!(read_snapshot(&repeated_auto).snapshot_id, first_auto_id);

        let (_, second_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("second")).unwrap();
        state.current_snapshot_context = Some(second_context);
        let second_auto = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();
        assert_ne!(first_auto, second_auto);

        let catalog = list_session_snapshots_in_dir(temp.path()).unwrap();
        assert_eq!(catalog.manual.len(), 2);
        assert_eq!(catalog.automatic.len(), 2);
        assert!(catalog
            .automatic
            .iter()
            .any(|entry| entry.base_manual_name.as_deref() == Some("second")));
    }

    #[test]
    fn named_load_matches_manual_snapshots_only() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("named load");
        let (_, manual_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("checkpoint")).unwrap();
        state.current_snapshot_context = Some(manual_context);
        let auto_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();
        let mut auto = read_snapshot(&auto_path);
        auto.name = "checkpoint".to_string();
        save_snapshot_at_path(&auto_path, &auto).unwrap();

        let matches = load_snapshot_in_dir(temp.path(), "checkpoint").unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1.kind, SnapshotKind::Manual);
    }

    #[test]
    fn loaded_auto_snapshot_overwrites_itself() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("unbound work");
        let first_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();
        let first = read_snapshot(&first_path);
        state.current_snapshot_context = Some(SnapshotContext::from_snapshot(
            snapshot_key_from_path(&first_path).unwrap().to_string(),
            &first,
        ));
        state.chat_state.messages.push(Message::system("continued"));

        let second_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();

        assert_eq!(first_path, second_path);
        assert_eq!(json_file_count(temp.path()), 1);
        assert_eq!(read_snapshot(&second_path).snapshot_id, first.snapshot_id);
    }

    #[test]
    fn explicit_same_name_overwrites_exact_nested_manual_and_preserves_id() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("manual branches");
        let (_, root_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("root")).unwrap();
        state.current_snapshot_context = Some(root_context);
        let (child_path, child_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("child")).unwrap();
        let child_id = child_context.snapshot_id.clone();
        state.current_snapshot_context = Some(child_context);
        state
            .chat_state
            .messages
            .push(Message::system("updated child"));

        let (overwritten_path, overwritten_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("child")).unwrap();

        assert_eq!(child_path, overwritten_path);
        assert_eq!(overwritten_context.snapshot_id, child_id);
        assert_eq!(overwritten_context.parent_chain, vec!["root".to_string()]);
        assert!(!temp.path().join("child.json").exists());
    }

    #[test]
    fn save_from_auto_uses_its_manual_source_as_anchor() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("resume and checkpoint");
        let (manual_path, manual_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("base")).unwrap();
        state.current_snapshot_context = Some(manual_context);
        let auto_path = autosave_on_interrupt_in_dir(temp.path(), &state, None)
            .unwrap()
            .unwrap();
        let auto = read_snapshot(&auto_path);
        state.current_snapshot_context = Some(SnapshotContext::from_snapshot(
            snapshot_key_from_path(&auto_path).unwrap().to_string(),
            &auto,
        ));

        let (updated_base, _) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("base")).unwrap();
        assert_eq!(updated_base, manual_path);

        let (_, auto_context) =
            load_snapshot_by_key_in_dir(temp.path(), snapshot_key_from_path(&auto_path).unwrap())
                .unwrap();
        assert!(auto_context.is_empty());
        let auto = read_snapshot(&auto_path);
        state.current_snapshot_context = Some(SnapshotContext::from_snapshot(
            snapshot_key_from_path(&auto_path).unwrap().to_string(),
            &auto,
        ));
        let (branch_path, branch_context) =
            save_manual_snapshot_in_dir(temp.path(), &state, None, Some("branch")).unwrap();
        assert_eq!(branch_path.file_name().unwrap(), "base_branch.json");
        assert_eq!(branch_context.parent_chain, vec!["base".to_string()]);
    }

    #[test]
    fn unnamed_manual_saves_generate_unique_names() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state_with_prompt("same second");
        let (_, first) = save_manual_snapshot_in_dir(temp.path(), &state, None, None).unwrap();
        state.current_snapshot_context = Some(first.clone());
        let (_, second) = save_manual_snapshot_in_dir(temp.path(), &state, None, None).unwrap();

        assert_ne!(first.name, second.name);
        assert!(second.name.starts_with(&format!("{}-", first.name)));
    }

    #[test]
    fn legacy_v1_snapshots_are_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let legacy_path = temp.path().join("legacy.json");
        fs::write(
            &legacy_path,
            r#"{"version":1,"saved_at_ms":1,"session_id":"legacy","workspace":""}"#,
        )
        .unwrap();
        let before = fs::read(&legacy_path).unwrap();

        let catalog = list_session_snapshots_in_dir(temp.path()).unwrap();
        assert!(catalog.is_empty());
        assert!(load_snapshot_in_dir(temp.path(), "legacy").is_err());
        let state = state_with_prompt("do not touch legacy data");
        assert!(save_manual_snapshot_in_dir(temp.path(), &state, None, Some("legacy")).is_err());
        assert_eq!(fs::read(&legacy_path).unwrap(), before);
    }

    #[test]
    fn dialog_defaults_to_manual_and_toggles_non_empty_panes() {
        fn entry(kind: SnapshotKind, name: &str) -> SessionSnapshotListEntry {
            SessionSnapshotListEntry {
                kind,
                name: name.to_string(),
                snapshot_key: name.to_string(),
                saved_at_ms: 0,
                parent_name: None,
                parent_chain: Vec::new(),
                depth: 0,
                base_manual_name: None,
            }
        }

        let mut dialog = SessionSnapshotDialog::new(SessionSnapshotCatalog {
            manual: vec![
                entry(SnapshotKind::Manual, "manual-1"),
                entry(SnapshotKind::Manual, "manual-2"),
            ],
            automatic: vec![entry(SnapshotKind::Auto, "auto")],
        });
        assert_eq!(dialog.active_pane, SessionSnapshotPane::Manual);
        dialog.move_down();
        assert_eq!(dialog.selected_entry().unwrap().name, "manual-2");
        for _ in 0..10 {
            dialog.move_down();
        }
        assert_eq!(dialog.manual_selected, 1);
        dialog.toggle_pane();
        assert_eq!(dialog.active_pane, SessionSnapshotPane::Automatic);
        assert_eq!(dialog.selected_entry().unwrap().name, "auto");

        let automatic_only = SessionSnapshotDialog::new(SessionSnapshotCatalog {
            manual: Vec::new(),
            automatic: vec![entry(SnapshotKind::Auto, "auto")],
        });
        assert_eq!(automatic_only.active_pane, SessionSnapshotPane::Automatic);
    }
}
