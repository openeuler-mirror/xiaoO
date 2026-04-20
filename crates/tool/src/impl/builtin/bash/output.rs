use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BashOutput {
    pub stdout: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    pub stderr: String,
    #[serde(default)]
    pub stderr_truncated: bool,
    pub exit_code: Option<i32>,
    pub interrupted: bool,
}
