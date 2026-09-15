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
