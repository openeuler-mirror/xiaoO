use agent_contracts::runtime::RuntimeView;
use agent_contracts::trace::{TraceOutcome, TraceSpanKind};
use agent_types::tool::{RawToolOutcome, ToolExecutionResult};
use serde_json::json;
use std::borrow::Cow;

use super::state::ToolExecutionState;
use super::ToolCallImpl;

impl ToolCallImpl {
    pub(super) async fn begin_trace_span(
        &self,
        state: &mut ToolExecutionState,
        runtime: &dyn RuntimeView,
    ) {
        let span = runtime
            .trace_recorder()
            .begin_span(
                TraceSpanKind::ToolCall,
                Cow::Borrowed("tool_call"),
                json!({
                    "call_id": state.final_call.call_id,
                    "tool_name": state.final_call.tool_name,
                    "agent_id": runtime.agent_context().metadata().agent_id,
                    "model": runtime.agent_context().metadata().model,
                    "session_id": runtime.agent_context().metadata().session_id,
                    "effective_input": state.final_call.input,
                }),
            )
            .await;
        state.trace_span = Some(span);
    }

    pub(super) async fn update_trace_span(
        &self,
        state: &ToolExecutionState,
        runtime: &dyn RuntimeView,
        phase: &'static str,
    ) {
        if let Some(span) = state.trace_span.as_ref() {
            runtime
                .trace_recorder()
                .update_span(
                    span,
                    json!({
                        "phase": phase,
                        "pre_hook_count": state.pre_hook_results.len(),
                        "post_hook_count": state.post_hook_results.len(),
                        "error_hook_count": state.error_hook_results.len(),
                        "effective_input": state.final_call.input,
                    }),
                )
                .await;
        }
    }

    pub(super) async fn end_trace_span(
        &self,
        state: &mut ToolExecutionState,
        runtime: &dyn RuntimeView,
        outcome: TraceOutcome,
        result: &ToolExecutionResult,
        phase: &'static str,
    ) {
        let Some(span) = state.trace_span.take() else {
            return;
        };

        let (result_kind, execution_error) = match result {
            ToolExecutionResult::Completed { raw_outcome, .. } => match raw_outcome {
                RawToolOutcome::Success { .. } => ("success", None),
                RawToolOutcome::Error { message } => ("tool_error", Some(message.clone())),
            },
            ToolExecutionResult::Suspended { .. } => ("suspended", None),
            ToolExecutionResult::Denied { error, .. } => {
                ("denied", error.as_ref().map(ToString::to_string))
            }
            ToolExecutionResult::Failed {
                execution_error, ..
            } => ("failed", Some(execution_error.to_string())),
        };

        runtime
            .trace_recorder()
            .end_span(
                span,
                outcome,
                merge_trace_end_fields(
                    json!({
                        "phase": phase,
                        "result_kind": result_kind,
                        "pre_hook_count": state.pre_hook_results.len(),
                        "post_hook_count": state.post_hook_results.len(),
                        "error_hook_count": state.error_hook_results.len(),
                        "execution_error": execution_error,
                    }),
                    result,
                ),
            )
            .await;
    }
}

fn merge_trace_end_fields(
    base: serde_json::Value,
    result: &ToolExecutionResult,
) -> serde_json::Value {
    let extra = match result {
        ToolExecutionResult::Completed { raw_outcome, .. } => match raw_outcome {
            RawToolOutcome::Success { output } => json!({
                "final_output": output,
            }),
            RawToolOutcome::Error { message } => json!({
                "final_output": message,
            }),
        },
        ToolExecutionResult::Suspended { suspend_token, .. } => json!({
            "suspend_token": suspend_token,
        }),
        ToolExecutionResult::Failed {
            execution_error, ..
        } => json!({
            "execution_error": execution_error.to_string(),
        }),
        ToolExecutionResult::Denied { error, .. } => json!({
            "execution_error": error.as_ref().map(ToString::to_string),
        }),
    };

    match (base, extra) {
        (serde_json::Value::Object(mut base_map), serde_json::Value::Object(extra_map)) => {
            for (key, value) in extra_map {
                base_map.insert(key, value);
            }
            serde_json::Value::Object(base_map)
        }
        (base_value, _) => base_value,
    }
}
