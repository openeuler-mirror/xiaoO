use agent_contracts::runtime::RuntimeView;
use agent_types::events::ToolLifecycleEvent;
use agent_types::tool::{
    FinalToolCall, ToolExecutionError, ToolExecutionResult, ToolExecutorOutput,
    ToolLifecycleRecord, ToolLifecycleStatus,
};
use std::time::{SystemTime, UNIX_EPOCH};

use super::state::{ExecutorPhaseResult, ToolExecutionState};
use super::ToolCallImpl;

impl ToolCallImpl {
    pub(super) fn load_execution_context(
        &self,
        final_call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolLifecycleRecord, ToolExecutionError> {
        Ok(runtime.state_store().begin(final_call, self.spec.as_ref()))
    }

    pub(super) fn mark_lifecycle_running(
        &self,
        state: &mut ToolExecutionState,
        runtime: &dyn RuntimeView,
    ) {
        if let Some(record) = state.lifecycle_record.as_mut() {
            record.status = ToolLifecycleStatus::Running;
            runtime.state_store().update(record);
        }
    }

    pub(super) fn persist_terminal_lifecycle(
        &self,
        state: &mut ToolExecutionState,
        result: &ToolExecutionResult,
        runtime: &dyn RuntimeView,
    ) {
        let Some(record) = state.lifecycle_record.as_mut() else {
            return;
        };

        record.finished_at_ms = Some(current_time_ms());

        match result {
            ToolExecutionResult::Completed { .. } => {
                record.status = ToolLifecycleStatus::Completed;
                runtime.state_store().finish(record, result);
            }
            ToolExecutionResult::Suspended { .. } => {
                record.status = ToolLifecycleStatus::Suspended;
                runtime.state_store().finish(record, result);
            }
            ToolExecutionResult::Denied { .. } => {
                record.status = ToolLifecycleStatus::Denied;
                runtime.state_store().finish(record, result);
            }
            ToolExecutionResult::Failed {
                execution_error, ..
            } => {
                record.status = ToolLifecycleStatus::Failed;
                runtime.state_store().fail(record, execution_error);
            }
        }
    }

    pub(super) async fn invoke_resolved_executor(
        &self,
        final_call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ExecutorPhaseResult, ToolExecutionError> {
        match self.executor.invoke(final_call, runtime).await {
            Ok(ToolExecutorOutput::Completed { raw_outcome }) => {
                Ok(ExecutorPhaseResult::Completed(raw_outcome))
            }
            Ok(ToolExecutorOutput::Suspended { suspend_token }) => {
                Ok(ExecutorPhaseResult::Suspended(suspend_token))
            }
            Err(execution_error) => Ok(ExecutorPhaseResult::Failed(execution_error)),
        }
    }

    pub(super) fn emit_tool_event(&self, runtime: &dyn RuntimeView, event: ToolLifecycleEvent) {
        runtime.tool_events().emit(event);
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
