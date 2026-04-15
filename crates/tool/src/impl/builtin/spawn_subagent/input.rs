use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SpawnSubagentInput {
    pub description: String,
    pub task_goal: String,
    pub task_context: String,
    pub output_schema: serde_json::Value,
}
