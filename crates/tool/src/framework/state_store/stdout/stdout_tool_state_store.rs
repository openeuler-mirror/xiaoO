use agent_contracts::tool::{ToolSpecView, ToolStateStore};
use agent_types::tool::{
    FinalToolCall, ToolExecutionError, ToolExecutionResult, ToolLifecycleRecord,
    ToolLifecycleStatus,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct PrintStdoutStateStore;

impl PrintStdoutStateStore {
    pub fn new() -> Self {
        Self
    }
}

impl ToolStateStore for PrintStdoutStateStore {
    fn begin(&self, call: &FinalToolCall, spec: &dyn ToolSpecView) -> ToolLifecycleRecord {
        let record = ToolLifecycleRecord {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            status: ToolLifecycleStatus::Pending,
            started_at_ms: current_time_ms(),
            finished_at_ms: None,
        };

        println!(
            "[state_store.begin] call_id={} tool_name={} spec_id={}",
            call.call_id,
            call.tool_name,
            spec.id()
        );

        record
    }

    fn update(&self, record: &ToolLifecycleRecord) {
        println!(
            "[state_store.update] call_id={} tool_name={} status={:?}",
            record.call_id, record.tool_name, record.status
        );
    }

    fn finish(&self, record: &ToolLifecycleRecord, result: &ToolExecutionResult) {
        println!(
            "[state_store.finish] call_id={} tool_name={} result={:?}",
            record.call_id, record.tool_name, result
        );
    }

    fn fail(&self, record: &ToolLifecycleRecord, error: &ToolExecutionError) {
        eprintln!(
            "[state_store.fail] call_id={} tool_name={} error={}",
            record.call_id, record.tool_name, error
        );
    }

    fn store_type(&self) -> &'static str {
        "stdout"
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
