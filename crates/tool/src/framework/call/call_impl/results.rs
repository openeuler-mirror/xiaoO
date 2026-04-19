use agent_types::tool::{
    FinalToolCall, PreHookResult, RawToolOutcome, ToolExecutionError, ToolExecutionResult,
};

use super::state::ToolExecutionState;
use super::ToolCallImpl;

impl ToolCallImpl {
    pub(super) fn build_denied_result(&self, state: &ToolExecutionState) -> ToolExecutionResult {
        let error = state.execution_error.clone().or_else(|| {
            state
                .pre_hook_results
                .iter()
                .find_map(|result| match result {
                    PreHookResult::Deny { reason } => Some(ToolExecutionError::PermissionDenied {
                        message: reason.clone(),
                    }),
                    _ => None,
                })
        });

        ToolExecutionResult::Denied {
            final_call: state.final_call.clone(),
            pre_hook_results: state.pre_hook_results.clone(),
            error_hook_results: state.error_hook_results.clone(),
            error,
        }
    }

    pub(super) fn build_failed_result(
        &self,
        state: &ToolExecutionState,
        execution_error: ToolExecutionError,
    ) -> ToolExecutionResult {
        ToolExecutionResult::Failed {
            final_call: state.final_call.clone(),
            pre_hook_results: state.pre_hook_results.clone(),
            error_hook_results: state.error_hook_results.clone(),
            execution_error,
        }
    }

    pub(super) fn build_completed_result(
        &self,
        state: &ToolExecutionState,
        raw_outcome: RawToolOutcome,
    ) -> ToolExecutionResult {
        ToolExecutionResult::Completed {
            final_call: state.final_call.clone(),
            raw_outcome,
            pre_hook_results: state.pre_hook_results.clone(),
            post_hook_results: state.post_hook_results.clone(),
        }
    }

    pub(super) fn build_suspended_result(
        &self,
        state: &ToolExecutionState,
        suspend_token: String,
    ) -> ToolExecutionResult {
        ToolExecutionResult::Suspended {
            final_call: state.final_call.clone(),
            pre_hook_results: state.pre_hook_results.clone(),
            suspend_token,
        }
    }
}

pub(super) fn format_tool_args_preview(call: &FinalToolCall) -> String {
    serde_json::to_string_pretty(&call.input).unwrap_or_else(|_| call.input.to_string())
}

pub(super) fn denied_reason(state: &ToolExecutionState) -> String {
    state
        .pre_hook_results
        .iter()
        .find_map(|result| match result {
            PreHookResult::Deny { reason } => Some(reason.clone()),
            _ => None,
        })
        .or_else(|| state.execution_error.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| "tool execution denied".to_string())
}

pub(super) fn result_error_message(result: &ToolExecutionResult) -> String {
    match result {
        ToolExecutionResult::Failed {
            execution_error, ..
        } => execution_error.to_string(),
        ToolExecutionResult::Denied { error, .. } => error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "tool execution denied".to_string()),
        _ => String::new(),
    }
}
