use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillSummary {
    pub skill_id: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemorySnippet {
    pub source: String,
    pub content: String,
    pub relevance_score: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub model: String,
    pub cwd: String,
    pub workspace_root: Option<String>,
    pub date: String,
    pub agent_id: String,
}
