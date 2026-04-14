use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSeverity {
    Normal,
    Warning,
    AutoCompact,
    Blocking,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextAnalysis {
    pub severity: ContextSeverity,
    pub estimated_tokens: usize,
    pub should_compact: bool,
    pub total_tokens: usize,
    pub available_tokens: usize,
    pub usage_ratio: f64,
}

impl ContextAnalysis {
    pub fn needs_compression(&self) -> bool {
        self.should_compact
    }
}
