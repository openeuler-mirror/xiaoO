mod control;
mod coordinator;
mod state;
mod types;

pub use control::{HostAction, SubagentHost};
pub use coordinator::{JoinDecision, SpawnDecision, SubagentCoordinator};
pub use state::{
    JoinRecord, JoinStatus, SubagentMailboxItem, SubagentRecord, SubagentSessionState,
    SubagentStatus, SubagentTerminalKind, SubagentTerminalSnapshot,
};
pub use types::{
    JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControl, SubagentControlError,
};
