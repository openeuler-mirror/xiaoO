use async_trait::async_trait;

use crate::state::{SubagentMailboxItem, SubagentTerminalSnapshot};
use crate::types::SubagentControlError;
use agent_types::common::ids::AgentId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAction {
    SpawnWorker {
        agent_id: AgentId,
        parent_agent_id: AgentId,
        description: String,
        prompt: String,
        output_schema: Option<serde_json::Value>,
        max_turns: Option<u32>,
    },
    SuspendWaiter {
        join_id: String,
        waiter_agent_id: AgentId,
        target_agent_id: AgentId,
    },
    WakeWaiter {
        join_id: String,
        waiter_agent_id: AgentId,
        terminal: SubagentTerminalSnapshot,
    },
    EnqueueMailboxItem {
        item: SubagentMailboxItem,
    },
}

#[async_trait]
pub trait SubagentHost: Send + Sync {
    async fn apply_host_actions(
        &self,
        session_id: &str,
        actions: Vec<HostAction>,
    ) -> Result<(), SubagentControlError>;
}
