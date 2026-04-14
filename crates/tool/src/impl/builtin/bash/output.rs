use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BashOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub interrupted: bool,
}
