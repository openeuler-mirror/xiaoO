use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SpawnSubagentInput {
    pub description: String,
    pub task_goal: String,
    pub task_context: String,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub subagent_role_id: Option<String>,
}
