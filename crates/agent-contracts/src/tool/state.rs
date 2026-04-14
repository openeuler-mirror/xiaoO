use crate::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{ToolExecutionError, ToolExecutionResult};
use agent_types::tool::ToolLifecycleRecord;

pub trait ToolStateStore: Send + Sync {
    fn begin(&self, call: &FinalToolCall, spec: &dyn ToolSpecView) -> ToolLifecycleRecord;
    fn update(&self, record: &ToolLifecycleRecord);
    fn finish(&self, record: &ToolLifecycleRecord, result: &ToolExecutionResult);
    fn fail(&self, record: &ToolLifecycleRecord, error: &ToolExecutionError);
    fn store_type(&self) -> &'static str {
        "unknown"
    }
}
