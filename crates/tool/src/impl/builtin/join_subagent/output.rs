use serde::{Deserialize, Serialize};
use subagent::SubagentTerminalSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JoinSubagentOutput {
    pub terminal: SubagentTerminalSnapshot,
}
