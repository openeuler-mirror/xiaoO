use agent_contracts::tool::{ToolSpecView, ToolStateStore};
use agent_types::tool::{
    FinalToolCall, ToolExecutionError, ToolExecutionResult, ToolLifecycleRecord,
    ToolLifecycleStatus,
};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
