use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub agent_id: String,
    pub model: String,
    pub session_id: Option<String>,
}
