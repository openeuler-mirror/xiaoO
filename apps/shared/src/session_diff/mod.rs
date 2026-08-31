use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod daemon_sinks;
pub use daemon_sinks::{DiffComputingLoopSink, DiffComputingToolSink, SessionDiffForwarder};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFileChangeStats {
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFileChangeEntry {
    pub file_path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeDelta {
    pub file_path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolFileBaseline {
    file_path: String,
    absolute_path: PathBuf,
}

/// Tracks per-file additions/deletions for a session, derived from tool
/// lifecycle events. The computation logic is identical whether the tracker
/// runs inside the TUI (local mode) or inside the daemon (remote mode); in
/// remote mode the daemon serializes the computed deltas and the TUI applies
/// them via [`SessionDiffTracker::apply_remote_delta`].
pub struct SessionDiffTracker {
    workspace: PathBuf,
    session_file_changes: BTreeMap<String, SessionFileChangeStats>,
    tool_file_changes: HashMap<String, FileChangeDelta>,
    tool_file_baselines: HashMap<String, ToolFileBaseline>,
    session_file_content_baselines: HashMap<String, Option<String>>,
}

impl SessionDiffTracker {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            session_file_changes: BTreeMap::new(),
            tool_file_changes: HashMap::new(),
            tool_file_baselines: HashMap::new(),
            session_file_content_baselines: HashMap::new(),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Update the workspace used for prefix stripping in
    /// [`Self::display_file_path`] and for resolving absolute paths in
    /// [`Self::capture_tool_file_baseline`]. Callers should invoke this
    /// whenever the owning `AppState.workspace` is mutated (e.g. after a
    /// `cd` command or snapshot restore), so that file paths in the session
    /// diff panel stay consistent with the active workspace.
    pub fn set_workspace(&mut self, workspace: PathBuf) {
        self.workspace = workspace;
    }

    pub fn session_file_changes(&self) -> &BTreeMap<String, SessionFileChangeStats> {
        &self.session_file_changes
    }

    pub fn clear(&mut self) {
        self.session_file_changes.clear();
        self.tool_file_changes.clear();
        self.tool_file_baselines.clear();
        self.session_file_content_baselines.clear();
    }

    pub fn restore(&mut self, snapshot: BTreeMap<String, SessionFileChangeStats>) {
        self.session_file_changes = snapshot;
        self.tool_file_changes.clear();
        self.tool_file_baselines.clear();
        self.session_file_content_baselines.clear();
    }

    /// High-level entry: tool transitioned to Running. Captures the file's
    /// current content as a baseline so that on completion a real content
    /// diff can be computed.
    pub fn on_tool_running(&mut self, call_id: &str, tool: &str, args_preview: &str) {
        self.capture_tool_file_baseline(call_id, tool, args_preview);
    }

    /// High-level entry: tool transitioned to Completed. Returns the computed
    /// delta so the daemon (remote mode) can forward it to the TUI; the local
    /// caller may simply discard the return value.
    pub fn on_tool_completed(
        &mut self,
        call_id: &str,
        tool: &str,
        args_preview: &str,
        file_change: Option<FileChangeDelta>,
    ) -> Option<FileChangeDelta> {
        let fallback = file_change.or_else(|| file_change_delta_from_tool_args(tool, args_preview));
        self.reconcile_tool_file_change_from_baseline(call_id, fallback)
    }

    /// High-level entry: tool transitioned to Failed. Discards any baseline and
    /// optionally applies an externally-supplied delta. Returns the next delta
    /// stored for this `call_id` (typically `None` for a failed call, but a
    /// non-`None` `file_change` argument may still produce one).
    pub fn on_tool_failed(
        &mut self,
        call_id: &str,
        file_change: Option<FileChangeDelta>,
    ) -> Option<FileChangeDelta> {
        self.discard_tool_file_baseline(call_id);
        self.reconcile_tool_file_change(call_id, file_change)
    }

    /// Remote-mode entry: directly apply a delta precomputed by the daemon.
    /// Skips baseline/args computation entirely.
    pub fn apply_remote_delta(&mut self, call_id: &str, delta: FileChangeDelta) {
        self.reconcile_tool_file_change(call_id, Some(delta));
    }

    pub(crate) fn reconcile_tool_file_change(
        &mut self,
        call_id: &str,
        next: Option<FileChangeDelta>,
    ) -> Option<FileChangeDelta> {
        if let Some(previous) = self.tool_file_changes.remove(call_id) {
            self.adjust_session_file_change(
                &previous.file_path,
                previous.additions,
                previous.deletions,
                false,
            );
        }

        let Some(next) = next.filter(|change| change.additions > 0 || change.deletions > 0) else {
            return None;
        };

        self.adjust_session_file_change(&next.file_path, next.additions, next.deletions, true);
        self.tool_file_changes
            .insert(call_id.to_string(), next.clone());
        Some(next)
    }

    pub(crate) fn capture_tool_file_baseline(
        &mut self,
        call_id: &str,
        tool: &str,
        args_preview: &str,
    ) {
        if self.tool_file_baselines.contains_key(call_id) {
            return;
        }
        let Some(file_path) = parse_tool_target_file_path(tool, args_preview) else {
            return;
        };
        let absolute_path = resolve_workspace_file_path(&self.workspace, &file_path);
        let baseline_content = match read_file_baseline(&absolute_path) {
            // File cannot be read (permission denied, I/O error, ...).
            // Without a reliable baseline we cannot content-diff this call;
            // skip baseline capture so `on_tool_completed` falls back to
            // args-based estimation instead of producing wrong counts.
            FileReadOutcome::Unreadable => return,
            FileReadOutcome::Absent => None,
            FileReadOutcome::Present(content) => Some(content),
        };
        self.session_file_content_baselines
            .entry(file_path.clone())
            .or_insert_with(|| baseline_content.clone());
        self.tool_file_baselines.insert(
            call_id.to_string(),
            ToolFileBaseline {
                file_path,
                absolute_path,
            },
        );
    }

    pub(crate) fn reconcile_tool_file_change_from_baseline(
        &mut self,
        call_id: &str,
        fallback: Option<FileChangeDelta>,
    ) -> Option<FileChangeDelta> {
        let Some(baseline) = self.tool_file_baselines.remove(call_id) else {
            if !self.tool_file_changes.contains_key(call_id) {
                return self.reconcile_tool_file_change(call_id, fallback);
            }
            return None;
        };

        let current_content = read_file_baseline(&baseline.absolute_path);
        let initial_content = self.session_file_content_baselines.get(&baseline.file_path);
        let computed = match current_content {
            // The baseline was captured successfully (the file existed and
            // was readable before the tool ran), but the post-tool read
            // failed (permission change, I/O error, ...). Computing a
            // content delta from `(Some, None)` would wrongly report every
            // baseline line as deleted. Instead, abandon content diffing
            // and fall through to the args-based `fallback`.
            FileReadOutcome::Unreadable => None,
            FileReadOutcome::Absent => initial_content.and_then(|initial| {
                if initial.is_none() {
                    // Both absent: no change.
                    None
                } else {
                    Some(file_content_delta(
                        &baseline.file_path,
                        initial.as_deref(),
                        None,
                    ))
                }
            }),
            FileReadOutcome::Present(ref current) => initial_content.and_then(|initial_ref| {
                let initial = initial_ref.as_deref();
                if initial.is_none() && current.is_empty() {
                    None
                } else {
                    Some(file_content_delta(
                        &baseline.file_path,
                        initial,
                        Some(current.as_str()),
                    ))
                }
            }),
        };
        if let Some(delta) = computed.or(fallback) {
            self.session_file_changes.insert(
                delta.file_path.clone(),
                SessionFileChangeStats {
                    additions: delta.additions,
                    deletions: delta.deletions,
                },
            );
            if delta.additions == 0 && delta.deletions == 0 {
                self.session_file_changes.remove(&delta.file_path);
            }
            Some(delta)
        } else {
            self.reconcile_tool_file_change(call_id, None)
        }
    }

    pub(crate) fn discard_tool_file_baseline(&mut self, call_id: &str) {
        self.tool_file_baselines.remove(call_id);
    }

    pub fn clear_tool_file_baselines(&mut self) {
        self.tool_file_baselines.clear();
    }

    /// Drop all per-turn transient state (per-call deltas and pending
    /// baselines) without touching the long-lived per-file change totals
    /// (`session_file_changes`) or the session-start content baselines
    /// (`session_file_content_baselines`). Intended to be called on
    /// `LoopEnd` so the per-call maps do not grow unboundedly across turns.
    pub fn clear_per_turn_state(&mut self) {
        self.tool_file_changes.clear();
        self.tool_file_baselines.clear();
    }

    pub fn sorted_session_file_changes(&self) -> Vec<SessionFileChangeEntry> {
        let mut entries = self
            .session_file_changes
            .iter()
            .map(|(file_path, stats)| SessionFileChangeEntry {
                file_path: file_path.clone(),
                additions: stats.additions,
                deletions: stats.deletions,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            let left_total = left.additions + left.deletions;
            let right_total = right.additions + right.deletions;
            right_total
                .cmp(&left_total)
                .then(right.additions.cmp(&left.additions))
                .then(left.file_path.cmp(&right.file_path))
        });
        entries
    }

    pub fn display_file_path(&self, file_path: &str) -> String {
        let path = Path::new(file_path);
        if let Ok(relative) = path.strip_prefix(&self.workspace) {
            let display = relative.display().to_string();
            if !display.is_empty() {
                return display;
            }
        }
        file_path.to_string()
    }

    fn adjust_session_file_change(
        &mut self,
        file_path: &str,
        additions: u32,
        deletions: u32,
        add: bool,
    ) {
        let entry = self
            .session_file_changes
            .entry(file_path.to_string())
            .or_default();
        if add {
            entry.additions = entry.additions.saturating_add(additions);
            entry.deletions = entry.deletions.saturating_add(deletions);
        } else {
            entry.additions = entry.additions.saturating_sub(additions);
            entry.deletions = entry.deletions.saturating_sub(deletions);
        }

        if entry.additions == 0 && entry.deletions == 0 {
            self.session_file_changes.remove(file_path);
        }
    }
}

pub(crate) fn parse_tool_target_file_path(tool: &str, args_preview: &str) -> Option<String> {
    match tool {
        "file_edit" | "file_write" => {
            let value: serde_json::Value = serde_json::from_str(args_preview).ok()?;
            value.get("file_path")?.as_str().map(ToOwned::to_owned)
        }
        _ => None,
    }
}

pub fn file_change_delta_from_tool_args(tool: &str, args_preview: &str) -> Option<FileChangeDelta> {
    let value: serde_json::Value = serde_json::from_str(args_preview).ok()?;
    let file_path = value.get("file_path")?.as_str()?.to_string();
    let (additions, deletions) = match tool {
        "file_edit" => {
            let old_string = value.get("old_string")?.as_str()?;
            let new_string = value.get("new_string")?.as_str()?;
            line_change_counts(old_string, new_string)
        }
        "file_write" => {
            let content = value.get("content")?.as_str()?;
            (text_line_count(content), 0)
        }
        _ => return None,
    };

    if additions == 0 && deletions == 0 {
        return None;
    }

    Some(FileChangeDelta {
        file_path,
        additions,
        deletions,
    })
}

fn resolve_workspace_file_path(workspace: &Path, file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

/// Outcome of reading a file for diff baseline purposes. Distinguishes
/// "file does not exist" (a legitimate state that produces a real diff)
/// from "file exists but cannot be read" (an error that should NOT be
/// treated as content, otherwise every baseline line would be wrongly
/// reported as deleted).
enum FileReadOutcome {
    Absent,
    Present(String),
    Unreadable,
}

fn read_file_baseline(path: &Path) -> FileReadOutcome {
    match fs::read_to_string(path) {
        Ok(content) => FileReadOutcome::Present(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileReadOutcome::Absent,
        Err(error) => {
            tracing::warn!(
                target: "session_diff",
                path = %path.display(),
                error = %error,
                "failed to read file for diff baseline; falling back to args estimation",
            );
            FileReadOutcome::Unreadable
        }
    }
}

fn file_content_delta(
    file_path: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> FileChangeDelta {
    let (additions, deletions) = match (before, after) {
        (Some(before), Some(after)) => line_change_counts(before, after),
        (None, Some(after)) => (text_line_count(after), 0),
        (Some(before), None) => (0, text_line_count(before)),
        (None, None) => (0, 0),
    };

    FileChangeDelta {
        file_path: file_path.to_string(),
        additions,
        deletions,
    }
}

fn line_change_counts(before: &str, after: &str) -> (u32, u32) {
    if before == after {
        return (0, 0);
    }

    let before_lines = text_lines(before);
    let after_lines = text_lines(after);
    if before_lines.is_empty() {
        return (after_lines.len() as u32, 0);
    }
    if after_lines.is_empty() {
        return (0, before_lines.len() as u32);
    }

    let cell_count = before_lines.len().saturating_mul(after_lines.len());
    let common = if cell_count > 20_000 {
        coarse_common_line_count(&before_lines, &after_lines)
    } else {
        lcs_line_count(&before_lines, &after_lines)
    };

    (
        after_lines.len().saturating_sub(common) as u32,
        before_lines.len().saturating_sub(common) as u32,
    )
}

fn lcs_line_count(before_lines: &[&str], after_lines: &[&str]) -> usize {
    let mut previous = vec![0usize; after_lines.len() + 1];
    let mut current = vec![0usize; after_lines.len() + 1];
    for before_line in before_lines {
        for (after_index, after_line) in after_lines.iter().enumerate() {
            current[after_index + 1] = if before_line == after_line {
                previous[after_index] + 1
            } else {
                current[after_index].max(previous[after_index + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(0);
    }
    previous[after_lines.len()]
}

fn coarse_common_line_count(before_lines: &[&str], after_lines: &[&str]) -> usize {
    let mut prefix = 0usize;
    while prefix < before_lines.len().min(after_lines.len())
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < before_lines.len().saturating_sub(prefix)
        && suffix < after_lines.len().saturating_sub(prefix)
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    prefix + suffix
}

fn text_line_count(text: &str) -> u32 {
    text_lines(text).len() as u32
}

fn text_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.lines().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker_in_temp_workspace() -> (tempfile::TempDir, SessionDiffTracker) {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let tracker = SessionDiffTracker::new(workspace);
        (temp, tracker)
    }

    #[test]
    fn running_then_completed_for_new_file_counts_addition() {
        let (_temp, mut tracker) = tracker_in_temp_workspace();
        tracker.on_tool_running(
            "call-1",
            "file_write",
            r#"{"file_path":"aaa","content":"aaabbb"}"#,
        );
        let delta = tracker
            .on_tool_completed(
                "call-1",
                "file_write",
                r#"{"file_path":"aaa","content":"aaabbb"}"#,
                None,
            )
            .expect("delta should be computed");
        assert_eq!(delta.file_path, "aaa");
        assert_eq!(delta.additions, 1);
        assert_eq!(delta.deletions, 0);

        let entries = tracker.sorted_session_file_changes();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].additions, 1);
        assert_eq!(entries[0].deletions, 0);
    }

    #[test]
    fn file_edit_existing_file_counts_addition_and_deletion() {
        let (temp, mut tracker) = tracker_in_temp_workspace();
        let file = temp.path().join("workspace/README.md");
        fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").expect("baseline");

        tracker.on_tool_running(
            "call-1",
            "file_edit",
            r#"{"file_path":"README.md","old_string":"three","new_string":"THREE"}"#,
        );
        fs::write(&file, "one\ntwo\nTHREE\nfour\nfive\n").expect("modified");
        let delta = tracker
            .on_tool_completed(
                "call-1",
                "file_edit",
                r#"{"file_path":"README.md","old_string":"three","new_string":"THREE"}"#,
                None,
            )
            .expect("delta should be computed");
        assert_eq!(delta.additions, 1);
        assert_eq!(delta.deletions, 1);
    }

    #[test]
    fn apply_remote_delta_directly_records_change() {
        let (_temp, mut tracker) = tracker_in_temp_workspace();
        tracker.apply_remote_delta(
            "remote-call-1",
            FileChangeDelta {
                file_path: "src/lib.rs".to_string(),
                additions: 5,
                deletions: 2,
            },
        );
        let entries = tracker.sorted_session_file_changes();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_path, "src/lib.rs");
        assert_eq!(entries[0].additions, 5);
        assert_eq!(entries[0].deletions, 2);
    }

    #[test]
    fn clear_per_turn_state_drops_per_call_maps_but_preserves_file_totals() {
        // Regression: in remote mode the TUI invokes `clear_per_turn_state`
        // from `finish_stream_done` (no `LoopEnd` SSE event is forwarded by
        // the daemon). The contract: per-call `tool_file_changes` and
        // `tool_file_baselines` must be emptied so they do not grow
        // unboundedly across turns, while long-lived per-file totals
        // (`session_file_changes`) must survive.
        let (_temp, mut tracker) = tracker_in_temp_workspace();
        tracker.apply_remote_delta(
            "call-1",
            FileChangeDelta {
                file_path: "src/a.rs".to_string(),
                additions: 3,
                deletions: 1,
            },
        );
        tracker.apply_remote_delta(
            "call-2",
            FileChangeDelta {
                file_path: "src/b.rs".to_string(),
                additions: 2,
                deletions: 0,
            },
        );
        // Populate a baseline entry as well (path is irrelevant for the
        // contract; we only need a non-empty `tool_file_baselines` map).
        tracker.capture_tool_file_baseline(
            "call-3",
            "file_write",
            r#"{"file_path":"src/c.rs","content":"new"}"#,
        );

        assert_eq!(tracker.tool_file_changes.len(), 2);
        assert_eq!(tracker.tool_file_baselines.len(), 1);
        assert_eq!(tracker.session_file_changes.len(), 2);

        tracker.clear_per_turn_state();

        assert!(tracker.tool_file_changes.is_empty());
        assert!(tracker.tool_file_baselines.is_empty());
        // Per-file totals must survive so the diff panel keeps showing the
        // session's cumulative changes after the turn ends.
        assert_eq!(tracker.session_file_changes.len(), 2);
        let entries = tracker.sorted_session_file_changes();
        assert_eq!(entries[0].file_path, "src/a.rs");
        assert_eq!(entries[0].additions, 3);
        assert_eq!(entries[0].deletions, 1);
        assert_eq!(entries[1].file_path, "src/b.rs");
        assert_eq!(entries[1].additions, 2);
        assert_eq!(entries[1].deletions, 0);
    }
}
