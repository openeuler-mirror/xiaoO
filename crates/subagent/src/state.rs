use agent_types::common::ids::AgentId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    MaxTurnsReached,
    BudgetExhausted,
}

impl SubagentStatus {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubagentTerminalKind {
    Completed,
    Failed,
    Cancelled,
    MaxTurnsReached,
    BudgetExhausted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SubagentTerminalSnapshot {
    pub status: SubagentTerminalKind,
    pub reply: Option<String>,
    pub error: Option<String>,
    pub completed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SubagentRecord {
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    pub status: SubagentStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub last_terminal: Option<SubagentTerminalSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinStatus {
    Pending,
    Satisfied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct JoinRecord {
    pub join_id: String,
    pub waiter_agent_id: AgentId,
    pub target_agent_id: AgentId,
    pub status: JoinStatus,
    pub created_at_ms: u64,
    #[serde(default)]
    pub resolved_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SubagentMailboxItem {
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub status: SubagentTerminalKind,
    pub summary: String,
    pub completed_at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SubagentSessionState {
    #[serde(default)]
    pub agents: BTreeMap<String, SubagentRecord>,
    #[serde(default)]
    pub joins: BTreeMap<String, JoinRecord>,
    #[serde(default)]
    pub mailbox: VecDeque<SubagentMailboxItem>,
}
