use agent_contracts::tool::{ToolSpecView, ToolStateStore};
use agent_types::current_time_ms;
use agent_types::tool::{
    FinalToolCall, ToolExecutionError, ToolExecutionResult, ToolLifecycleRecord,
    ToolLifecycleStatus,
};

pub struct NoOpToolStateStore;

impl NoOpToolStateStore {
    pub fn new() -> Self {
        Self
    }
}

impl ToolStateStore for NoOpToolStateStore {
    fn begin(&self, call: &FinalToolCall, _spec: &dyn ToolSpecView) -> ToolLifecycleRecord {
        ToolLifecycleRecord {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            status: ToolLifecycleStatus::Pending,
            started_at_ms: current_time_ms(),
            finished_at_ms: None,
        }
    }

    fn update(&self, _record: &ToolLifecycleRecord) {}

    fn finish(&self, _record: &ToolLifecycleRecord, _result: &ToolExecutionResult) {}

    fn fail(&self, _record: &ToolLifecycleRecord, _error: &ToolExecutionError) {}

    fn store_type(&self) -> &'static str {
        "noop"
    }
}
