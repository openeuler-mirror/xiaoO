use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

use super::spec::TodoWriteToolSpec;
use super::types::{TodoItem, TodoStatus, TodoWriteInput, TodoWriteOutput};

type TodoStore = HashMap<String, Vec<TodoItem>>;

static TODO_STORE: OnceLock<Mutex<TodoStore>> = OnceLock::new();

pub struct TodoWriteToolExecutor {
    spec: Arc<TodoWriteToolSpec>,
}

impl TodoWriteToolExecutor {
    pub fn new(spec: Arc<TodoWriteToolSpec>) -> Self {
        Self { spec }
    }
}

#[async_trait]
impl ToolExecutor for TodoWriteToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: TodoWriteInput =
            serde_json::from_value(call.input.clone()).map_err(|error| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("failed to parse todo_write input: {error}"),
                }
            })?;
        validate_todos(&input.todos)?;

        let todo_key = todo_key(runtime);
        let store = TODO_STORE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut store = store
            .lock()
            .map_err(|_| ToolExecutionError::ExecutionFailed {
                message: "todo store lock poisoned".to_string(),
            })?;

        let old_todos = store.get(&todo_key).cloned().unwrap_or_default();
        let all_done = input
            .todos
            .iter()
            .all(|todo| matches!(todo.status, TodoStatus::Completed));

        if all_done {
            store.remove(&todo_key);
        } else {
            store.insert(todo_key, input.todos.clone());
        }

        let output = TodoWriteOutput {
            old_todos,
            new_todos: input.todos,
            verification_nudge_needed: Some(false),
        };
        let output = serde_json::to_string(&output).map_err(|error| {
            ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize todo_write output: {error}"),
            }
        })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output },
        })
    }
}

fn todo_key(runtime: &dyn RuntimeView) -> String {
    let metadata = runtime.agent_context().metadata();
    metadata
        .session_id
        .clone()
        .filter(|session_id| !session_id.trim().is_empty())
        .unwrap_or_else(|| metadata.agent_id.clone())
}

fn validate_todos(todos: &[TodoItem]) -> Result<(), ToolExecutionError> {
    for (index, todo) in todos.iter().enumerate() {
        if todo.content.trim().is_empty() {
            return Err(ToolExecutionError::ExecutionFailed {
                message: format!("todo at index {index} has empty content"),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
