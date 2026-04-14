use agent_types::tool::{FinalToolCall, ToolExecutionResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopSuspendReason {
    ToolCall {
        tool_name: String,
        suspend_token: String,
    },
}

#[derive(Clone, Debug)]
pub struct SuspendedToolCall {
    pub final_call: FinalToolCall,
    pub reason: LoopSuspendReason,
}

pub enum LoopRunResult {
    Complete(agent_types::outcome::AgentOutcome),
    Suspended(SuspendedToolCall),
}

impl SuspendedToolCall {
    pub fn from_tool_result(result: &ToolExecutionResult) -> Option<Self> {
        match result {
            ToolExecutionResult::Suspended {
                final_call,
                suspend_token,
                ..
            } => Some(Self {
                final_call: final_call.clone(),
                reason: LoopSuspendReason::ToolCall {
                    tool_name: final_call.tool_name.clone(),
                    suspend_token: suspend_token.clone(),
                },
            }),
            _ => None,
        }
    }
}
