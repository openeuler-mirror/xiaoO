use serde::{Deserialize, Serialize};

use crate::{
    DurableMemory, FactMemory, InstructionMemory, PromptHistoryEntry, ScoredMemory,
    SessionMemorySummary, TaskMemory, TokenUsageBaseline,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecallQuery {
    pub max_instruction_count: usize,
    pub max_fact_count: usize,
    pub max_prompt_history_count: usize,
    pub include_session_memory: bool,
    pub include_durable_memory: bool,
    /// When set, triggers semantic (vector + keyword hybrid) search on the
    /// SemanticMemoryStore. Leave None to skip semantic recall.
    #[serde(default)]
    pub semantic_query: Option<String>,
    /// Max results for semantic search. Only used when `semantic_query` is Some.
    #[serde(default = "default_semantic_limit")]
    pub semantic_limit: usize,
}

fn default_semantic_limit() -> usize {
    10
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecallPacket {
    pub instructions: Vec<InstructionMemory>,
    pub facts: Vec<FactMemory>,
    pub task: Option<TaskMemory>,
    pub prompt_history: Vec<PromptHistoryEntry>,
    pub usage_baseline: Option<TokenUsageBaseline>,
    pub session_memory: Option<SessionMemorySummary>,
    pub durable_memories: Vec<DurableMemory>,
    /// Results from semantic (vector + keyword) search. Empty when no
    /// SemanticMemoryStore is configured or semantic_query is None.
    #[serde(default)]
    pub semantic_results: Vec<ScoredMemory>,
}
