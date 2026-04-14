use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BashInput {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}
