use agent_contracts::runtime::RuntimeView;
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_types::hook::{HookInvokeInput, HookInvokeMetadata, HookInvokeOutput, HookPointId};
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput, RawToolOutcome, ToolExecutionError,
};
use hook::{resolve_hook_point_category, HookPointCategory};
use serde_json::json;
use std::borrow::Cow;

use super::state::ToolExecutionState;
use super::ToolCallImpl;

impl ToolCallImpl {
    pub(super) async fn run_pre_hook_sequence(
        &self,
        state: &mut ToolExecutionState,
        runtime: &dyn RuntimeView,
    ) -> Result<Vec<PreHookResult>, ToolExecutionError> {
        let hook_point = self.build_tool_hook_point(runtime, &state.final_call.tool_name, "pre");
        let category = resolve_hook_point_category(&hook_point).map_err(|error| {
            ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to resolve pre-hook category for tool call '{}' (hook_point='{}'): {}",
                    state.final_call.call_id, hook_point.0, error
                ),
            }
        })?;

        if category != HookPointCategory::ToolPre {
            return Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "pre-hook sequence expected ToolPre category but got {:?} for hook point {}",
                    category, hook_point.0
                ),
            });
        }

        let mut hookers = runtime.hookers().list_for_hook_point(&hook_point);
        hookers.retain(|hooker| runtime.hookers().is_enabled(hooker.id()));
        hookers.sort_by(|left, right| left.id().0.cmp(&right.id().0));

        if hookers.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for hooker in hookers {
            let hook_span = runtime
                .trace_recorder()
                .begin_span(
                    TraceSpanKind::Hook,
                    Cow::Borrowed("tool_pre_hook"),
                    json!({
                        "hook_kind": "tool_pre",
                        "hooker_id": hooker.id().to_string(),
                        "hook_point": hook_point.0,
                        "tool_name": state.final_call.tool_name,
                        "call_id": state.final_call.call_id,
                    }),
                )
                .await;

            let input = HookInvokeInput::Pre {
                input: PreToolHookInput {
                    call: state.final_call.clone(),
                },
                metadata: hook_invoke_metadata(&hook_span),
            };

            let output = match hooker.invoke(input, runtime).await {
                Ok(output) => output,
                Err(error) => {
                    tracing::warn!(
                        hooker_id = %hooker.id(),
                        call_id = %state.final_call.call_id,
                        tool = %state.final_call.tool_name,
                        error = %error,
                        "pre-hook invoke failed"
                    );
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": error.to_string()}),
                        )
                        .await;
                    continue;
                }
            };

            let pre_result = match output {
                HookInvokeOutput::Pre(pre_result) => pre_result,
                other => {
                    tracing::warn!(
                        hooker_id = %hooker.id(),
                        call_id = %state.final_call.call_id,
                        output = ?other,
                        "pre-hooker returned non-pre output"
                    );
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": "unexpected output variant"}),
                        )
                        .await;
                    continue;
                }
            };

            match pre_result {
                PreHookResult::Allow => {
                    results.push(PreHookResult::Allow);
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "allow"}))
                        .await;
                }
                PreHookResult::Deny { reason } => {
                    let fields = json!({"result": "deny", "reason": &reason});
                    results.push(PreHookResult::Deny { reason });
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Denied, fields)
                        .await;
                    return Ok(results);
                }
                PreHookResult::Transform { modified_input } => {
                    state.final_call.input = modified_input;
                    results.push(PreHookResult::Transform {
                        modified_input: state.final_call.input.clone(),
                    });
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "transform"}))
                        .await;
                }
            }
        }

        Ok(results)
    }

    pub(super) fn build_tool_hook_point(
        &self,
        runtime: &dyn RuntimeView,
        tool_name: &str,
        stage: &str,
    ) -> HookPointId {
        let agent_id = &runtime.agent_context().metadata().agent_id;
        HookPointId(format!("{}.Tool.{}.{}", agent_id, tool_name, stage))
    }

    pub(super) async fn run_post_hook_sequence(
        &self,
        state: &mut ToolExecutionState,
        runtime: &dyn RuntimeView,
    ) -> Result<Vec<PostHookResult>, ToolExecutionError> {
        if state.raw_outcome.is_none() {
            return Ok(Vec::new());
        }

        let initial_outcome = match state.raw_outcome.as_ref() {
            Some(raw_outcome) => raw_outcome.clone(),
            None => unreachable!(),
        };

        let hook_point = self.build_tool_hook_point(runtime, &state.final_call.tool_name, "post");
        let category = resolve_hook_point_category(&hook_point).map_err(|error| {
            ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to resolve post-hook category for tool call '{}' (hook_point='{}'): {}",
                    state.final_call.call_id, hook_point.0, error
                ),
            }
        })?;

        if category != HookPointCategory::ToolPost {
            return Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "post-hook sequence expected ToolPost category but got {:?} for hook point {}",
                    category, hook_point.0
                ),
            });
        }

        let mut hookers = runtime.hookers().list_for_hook_point(&hook_point);
        hookers.retain(|hooker| runtime.hookers().is_enabled(hooker.id()));
        hookers.sort_by(|left, right| left.id().0.cmp(&right.id().0));

        if hookers.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for hooker in hookers {
            let current_outcome = state
                .raw_outcome
                .as_ref()
                .cloned()
                .unwrap_or_else(|| initial_outcome.clone());

            let hook_span = runtime
                .trace_recorder()
                .begin_span(
                    TraceSpanKind::Hook,
                    Cow::Borrowed("tool_post_hook"),
                    json!({
                        "hook_kind": "tool_post",
                        "hooker_id": hooker.id().to_string(),
                        "hook_point": hook_point.0,
                        "tool_name": state.final_call.tool_name,
                        "call_id": state.final_call.call_id,
                    }),
                )
                .await;

            let input = HookInvokeInput::Post {
                input: PostToolHookInput {
                    call: state.final_call.clone(),
                    outcome: current_outcome,
                },
                metadata: hook_invoke_metadata(&hook_span),
            };

            let output = match hooker.invoke(input, runtime).await {
                Ok(output) => output,
                Err(error) => {
                    tracing::warn!(
                        hooker_id = %hooker.id(),
                        call_id = %state.final_call.call_id,
                        tool = %state.final_call.tool_name,
                        error = %error,
                        "post-hook invoke failed"
                    );
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": error.to_string()}),
                        )
                        .await;
                    continue;
                }
            };

            let post_result = match output {
                HookInvokeOutput::Post(post_result) => post_result,
                other => {
                    let error = format!(
                        "post-hooker '{}' returned non-post output {:?} for tool call '{}'",
                        hooker.id(),
                        other,
                        state.final_call.call_id
                    );
                    tracing::warn!(
                        hooker_id = %hooker.id(),
                        call_id = %state.final_call.call_id,
                        output = ?other,
                        "post-hooker returned non-post output"
                    );
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Error, json!({"error": error}))
                        .await;
                    continue;
                }
            };

            match post_result {
                PostHookResult::Accept => {
                    results.push(PostHookResult::Accept);
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "accept"}))
                        .await;
                }
                PostHookResult::Transform { modified_output } => {
                    state.raw_outcome = Some(RawToolOutcome::Success {
                        output: modified_output.clone(),
                    });
                    results.push(PostHookResult::Transform { modified_output });
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "transform"}))
                        .await;
                }
            }
        }

        Ok(results)
    }

    pub(super) async fn run_error_hook_sequence(
        &self,
        state: &ToolExecutionState,
        execution_error: &ToolExecutionError,
        runtime: &dyn RuntimeView,
    ) -> Result<Vec<ErrorHookResult>, ToolExecutionError> {
        let hook_point = self.build_tool_hook_point(runtime, &state.final_call.tool_name, "error");
        let category = resolve_hook_point_category(&hook_point).map_err(|error| {
            ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to resolve error-hook category for tool call '{}' (hook_point='{}'): {}",
                    state.final_call.call_id, hook_point.0, error
                ),
            }
        })?;

        if category != HookPointCategory::ToolError {
            return Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "error-hook sequence expected ToolError category but got {:?} for hook point {}",
                    category, hook_point.0
                ),
            });
        }

        let mut hookers = runtime.hookers().list_for_hook_point(&hook_point);
        hookers.retain(|hooker| runtime.hookers().is_enabled(hooker.id()));
        hookers.sort_by(|left, right| left.id().0.cmp(&right.id().0));

        if hookers.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for hooker in hookers {
            let hook_span = runtime
                .trace_recorder()
                .begin_span(
                    TraceSpanKind::Hook,
                    Cow::Borrowed("tool_error_hook"),
                    json!({
                        "hook_kind": "tool_error",
                        "hooker_id": hooker.id().to_string(),
                        "hook_point": hook_point.0,
                        "tool_name": state.final_call.tool_name,
                        "call_id": state.final_call.call_id,
                        "execution_error": execution_error.to_string(),
                    }),
                )
                .await;

            let input = HookInvokeInput::Error {
                input: ErrorToolHookInput {
                    call: state.final_call.clone(),
                    error: execution_error.clone(),
                },
                metadata: hook_invoke_metadata(&hook_span),
            };

            let output = match hooker.invoke(input, runtime).await {
                Ok(output) => output,
                Err(error) => {
                    tracing::warn!(
                        hooker_id = %hooker.id(),
                        call_id = %state.final_call.call_id,
                        tool = %state.final_call.tool_name,
                        error = %error,
                        "error-hook invoke failed"
                    );
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": error.to_string()}),
                        )
                        .await;
                    continue;
                }
            };

            let error_result = match output {
                HookInvokeOutput::Error(error_result) => error_result,
                other => {
                    let error = format!(
                        "error-hooker '{}' returned non-error output {:?} for tool call '{}'",
                        hooker.id(),
                        other,
                        state.final_call.call_id
                    );
                    tracing::warn!(
                        hooker_id = %hooker.id(),
                        call_id = %state.final_call.call_id,
                        output = ?other,
                        "error-hooker returned non-error output"
                    );
                    runtime
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Error, json!({"error": error}))
                        .await;
                    continue;
                }
            };

            let (trace_outcome, result_label) = match &error_result {
                ErrorHookResult::Propagate => (TraceOutcome::Ok, "propagate"),
                ErrorHookResult::Recover { .. } => (TraceOutcome::Ok, "recover"),
            };
            runtime
                .trace_recorder()
                .end_span(hook_span, trace_outcome, json!({"result": result_label}))
                .await;

            results.push(error_result);
        }

        Ok(results)
    }

    pub(super) async fn collect_error_hook_results_after_begin(
        &self,
        state: &mut ToolExecutionState,
        execution_error: &ToolExecutionError,
        runtime: &dyn RuntimeView,
    ) {
        match self
            .run_error_hook_sequence(state, execution_error, runtime)
            .await
        {
            Ok(error_hook_results) => state.error_hook_results = error_hook_results,
            Err(error_hook_failure) => {
                tracing::warn!(
                    call_id = %state.final_call.call_id,
                    tool = %state.final_call.tool_name,
                    error = %error_hook_failure,
                    "error-hook phase failed after begin"
                );
            }
        }
    }
}

fn hook_invoke_metadata(hook_span: &TraceSpanHandle) -> HookInvokeMetadata {
    HookInvokeMetadata {
        trace_id: Some(hook_span.trace_id().to_string()),
        span_id: Some(hook_span.span_id().to_string()),
        parent_span_id: hook_span.parent_span_id().map(ToString::to_string),
    }
}
