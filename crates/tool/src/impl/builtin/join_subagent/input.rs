use agent_types::common::ids::AgentId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JoinSubagentInput {
    pub target_agent_id: AgentId,
}
