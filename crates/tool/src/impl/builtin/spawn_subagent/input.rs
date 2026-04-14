use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SpawnSubagentInput {
    pub description: String,
    pub prompt: String,
}
