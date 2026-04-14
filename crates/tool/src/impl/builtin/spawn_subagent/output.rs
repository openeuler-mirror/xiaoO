use agent_types::common::ids::AgentId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SpawnSubagentOutput {
    pub agent_id: AgentId,
}
