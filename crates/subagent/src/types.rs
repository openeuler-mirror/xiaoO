use agent_types::common::ids::AgentId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::state::SubagentTerminalSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SpawnSubagentRequest {
    pub session_id: String,
    pub parent_agent_id: AgentId,
    pub description: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SpawnSubagentResult {
    pub agent_id: AgentId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct JoinSubagentRequest {
    pub session_id: String,
    pub waiter_agent_id: AgentId,
    pub target_agent_id: AgentId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JoinSubagentResult {
    Ready { terminal: SubagentTerminalSnapshot },
    Pending { join_id: String },
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubagentControlError {
    #[error("subagent not found: {agent_id}")]
    AgentNotFound { agent_id: String },
    #[error("subagent session mismatch: expected '{expected}', got '{actual}'")]
    SessionMismatch { expected: String, actual: String },
    #[error("agent cannot join itself: {agent_id}")]
    SelfJoin { agent_id: String },
    #[error("waiter already waiting on another join: {agent_id}")]
    WaiterAlreadyWaiting { agent_id: String },
    #[error("target terminal snapshot missing: {agent_id}")]
    MissingTerminalSnapshot { agent_id: String },
    #[error("subagent state invalid: {message}")]
    InvalidState { message: String },
    #[error("subagent control unavailable: {message}")]
    Unavailable { message: String },
}

#[async_trait]
pub trait SubagentControl: Send + Sync {
    async fn spawn(
        &self,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError>;

    async fn join(
        &self,
        request: JoinSubagentRequest,
    ) -> Result<JoinSubagentResult, SubagentControlError>;
}
