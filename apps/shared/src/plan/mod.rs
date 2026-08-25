use serde::{Deserialize, Serialize};

mod daemon_sinks;
pub use daemon_sinks::{
    PlanComputingLoopSink, PlanForwarder, SubagentMetaComputingLoopSink, SubagentMetaForwarder,
};

// The wire-facing plan types live in the protocol crate; re-export here to
// preserve existing `xiaoo_shared::plan::` import paths.
pub use protocol::plan::{TodoDisplayStatus, TodoSnapshotItem};

/// Full plan snapshot, forwarded by the daemon in remote mode so the TUI
/// doesn't need to re-parse the `todo_write` tool's args.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoSnapshotUpdate {
    pub title: String,
    pub items: Vec<TodoSnapshotItem>,
}

/// Metadata for a spawned subagent lane, forwarded by the daemon in remote
/// mode so the TUI doesn't need to re-parse the `spawn_subagent` tool's args.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnSubagentMetadata {
    pub agent_id: String,
    pub parent_agent_id: Option<String>,
    pub title: String,
    pub description: String,
    pub task_goal: String,
}

/// Wire shape of the `todo_write` tool's `args_preview` JSON.
#[derive(Debug, Deserialize)]
struct TodoWriteArgs {
    todos: Vec<TodoWriteArgItem>,
}

#[derive(Debug, Deserialize)]
struct TodoWriteArgItem {
    content: String,
    status: TodoWriteArgStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TodoWriteArgStatus {
    Pending,
    InProgress,
    Completed,
}

/// Wire shape of the `spawn_subagent` tool's `args_preview` JSON.
#[derive(Debug, Deserialize)]
struct SpawnSubagentArgs {
    #[serde(default)]
    description: String,
    #[serde(default)]
    task_goal: String,
    #[serde(default)]
    task_context: String,
    #[serde(default)]
    subagent_role_id: Option<String>,
}

/// Parses the `todo_write` tool's `args_preview` JSON into a structured
/// snapshot suitable for the TUI plan panel. Returns `None` if the JSON is
/// invalid or empty (the remote mode fallback when the daemon has not
/// forwarded a `PlanUpdate` event).
pub fn todo_snapshot_from_tool_args(args_preview: &str) -> Option<TodoSnapshotUpdate> {
    let args: TodoWriteArgs = serde_json::from_str(args_preview).ok()?;
    let items = args
        .todos
        .into_iter()
        .filter_map(|item| {
            let content = item.content.trim();
            if content.is_empty() {
                return None;
            }
            let status = match item.status {
                TodoWriteArgStatus::Pending => TodoDisplayStatus::Pending,
                TodoWriteArgStatus::InProgress => TodoDisplayStatus::InProgress,
                TodoWriteArgStatus::Completed => TodoDisplayStatus::Completed,
            };
            Some(TodoSnapshotItem {
                status,
                content: content.to_string(),
            })
        })
        .collect::<Vec<_>>();

    Some(TodoSnapshotUpdate {
        title: "Current task list".to_string(),
        items,
    })
}

/// Parses the spawned child's `agent_id` out of the `spawn_subagent` tool's
/// output JSON (`{"agent_id": "..."}`).
pub fn parse_spawn_subagent_agent_id_from_detail(detail: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(detail.trim()).ok()?;
    value.get("agent_id")?.as_str().map(ToOwned::to_owned)
}

/// Parses the `spawn_subagent` tool's `args_preview` JSON and derives the
/// display metadata (title / description / task_goal) for the spawned lane.
/// `parent_agent_id` is supplied by the caller (the daemon) since the sink
/// does not see the parent agent id directly in the `LoopEventSink` API.
pub fn parse_spawn_subagent_metadata_from_args(
    args_preview: &str,
    parent_agent_id: Option<String>,
    fallback_agent_id: Option<&str>,
) -> Option<SpawnSubagentMetadata> {
    let agent_id = fallback_agent_id.map(ToOwned::to_owned)?;
    let args: SpawnSubagentArgs = serde_json::from_str(args_preview).ok()?;

    let title = (!args.description.is_empty())
        .then(|| args.description.clone())
        .or_else(|| {
            args.task_goal
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            args.subagent_role_id
                .as_deref()
                .map(|role| format!("Subagent {role}"))
        })
        .unwrap_or_else(|| format!("Subagent {agent_id}"));

    let description = if !args.description.is_empty() {
        args.description
    } else {
        args.task_context
    };

    Some(SpawnSubagentMetadata {
        agent_id,
        parent_agent_id,
        title,
        description,
        task_goal: args.task_goal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_todo_write_args_into_snapshot() {
        let snapshot = todo_snapshot_from_tool_args(
            r#"{"todos":[{"content":"a","status":"pending"},{"content":"b","status":"completed"}]}"#,
        )
        .expect("snapshot should parse");
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.items[0].status, TodoDisplayStatus::Pending);
        assert_eq!(snapshot.items[1].status, TodoDisplayStatus::Completed);
    }

    #[test]
    fn returns_none_on_empty_args() {
        assert!(todo_snapshot_from_tool_args("").is_none());
    }

    #[test]
    fn parses_spawn_subagent_metadata_preferring_description_as_title() {
        let meta = parse_spawn_subagent_metadata_from_args(
            r#"{"description":"Refactor tests","task_goal":"split suite","task_context":"ctx","subagent_role_id":"tester"}"#,
            Some("root".to_string()),
            Some("agent-xyz"),
        )
        .expect("metadata should parse");
        assert_eq!(meta.agent_id, "agent-xyz");
        assert_eq!(meta.parent_agent_id.as_deref(), Some("root"));
        assert_eq!(meta.title, "Refactor tests");
        assert_eq!(meta.description, "Refactor tests");
        assert_eq!(meta.task_goal, "split suite");
    }

    #[test]
    fn falls_back_to_task_goal_first_line_when_description_empty() {
        let meta = parse_spawn_subagent_metadata_from_args(
            r#"{"description":"","task_goal":"line one\nline two","task_context":"","subagent_role_id":null}"#,
            None,
            Some("agent-1"),
        )
        .expect("metadata should parse");
        assert_eq!(meta.title, "line one");
    }

    #[test]
    fn returns_none_when_agent_id_missing() {
        assert!(parse_spawn_subagent_metadata_from_args(
            r#"{"description":"x","task_goal":"y","task_context":""}"#,
            None,
            None,
        )
        .is_none());
    }

    #[test]
    fn parses_spawn_subagent_agent_id_from_detail() {
        assert_eq!(
            parse_spawn_subagent_agent_id_from_detail(r#"{"agent_id":"abc"}"#).as_deref(),
            Some("abc")
        );
        assert!(parse_spawn_subagent_agent_id_from_detail("").is_none());
    }
}
