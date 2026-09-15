use super::super::types::TodoStatus;
use super::*;

#[test]
fn rejects_empty_todo_content() {
    let error = validate_todos(&[TodoItem {
        id: None,
        content: "  ".to_string(),
        status: TodoStatus::Pending,
    }])
    .expect_err("empty content should fail");

    assert!(error.to_string().contains("empty content"));
}

#[test]
fn serializes_camel_case_output() {
    let output = TodoWriteOutput {
        old_todos: Vec::new(),
        new_todos: vec![TodoItem {
            id: Some("task-1".to_string()),
            content: "Check the wiring".to_string(),
            status: TodoStatus::InProgress,
        }],
        verification_nudge_needed: Some(false),
    };

    let json = serde_json::to_string(&output).expect("output should serialize");
    assert!(json.contains("oldTodos"));
    assert!(json.contains("newTodos"));
    assert!(json.contains("verificationNudgeNeeded"));
    assert!(json.contains("in_progress"));
}
