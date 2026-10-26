use agent_contracts::runtime::RuntimeView;
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_types::common::HookerId;
use agent_types::hook::{HookInvokeInput, HookInvokeMetadata, HookInvokePrimary, HookPointId};
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, FinalToolCall, PostHookResult, PostToolHookInput,
    PreHookResult, PreToolHookInput, RawToolOutcome, ToolExecutionError,
};
use hook::{escape_hook_point_segment, resolve_hook_point_category, HookPointCategory};
use serde_json::{json, Value};
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
        let mut hookers = runtime.hookers().list_for_hook_point(&hook_point);
        hookers.retain(|hooker| runtime.hookers().is_enabled(hooker.id()));
        hookers.sort_by(|left, right| left.id().0.cmp(&right.id().0));

        // `build_tool_hook_point` escapes dots in the agent and tool name
        // segments, so the point is always well formed and wildcard patterns
        // like `*.Tool.*.pre` match dotted tool names too. An empty match
        // therefore genuinely means "no enabled hooker is attached to this
        // tool phase" and the phase is skipped, including hook-free
        // deployments.
        if hookers.is_empty() {
            return Ok(Vec::new());
        }

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
                    call: hooker_visible_call(&state.final_call),
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
                    results.push(PreHookResult::Deny {
                        reason: format!(
                            "pre-hooker '{}' failed, tool call denied (fail-closed): {}",
                            hooker.id().0,
                            error
                        ),
                    });
                    return Ok(results);
                }
            };

            let pre_result = match output.primary {
                HookInvokePrimary::Pre(pre_result) => pre_result,
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
                    results.push(PreHookResult::Deny {
                        reason: format!(
                            "pre-hooker '{}' returned non-pre output, tool call denied (fail-closed)",
                            hooker.id().0
                        ),
                    });
                    return Ok(results);
                }
            };

            match pre_result {
                PreHookResult::Allow { extra } => {
                    accumulate_extra(&mut state.final_call, hooker.id(), extra);
                    results.push(PreHookResult::Allow { extra: None });
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
                PreHookResult::Transform {
                    modified_input,
                    extra,
                } => {
                    state.final_call.input = modified_input;
                    accumulate_extra(&mut state.final_call, hooker.id(), extra);
                    results.push(PreHookResult::Transform {
                        modified_input: state.final_call.input.clone(),
                        extra: None,
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
        // The agent id and tool name are free-form and may contain dots
        // (e.g. MCP tools like `mcp__srv__github.create_issue`); escaping
        // them keeps the point at exactly 4 segments so category resolution
        // succeeds and 4-segment wildcard patterns (`*.Tool.*.pre`) still
        // match. Exact registrations target the escaped spelling.
        HookPointId(format!(
            "{}.Tool.{}.{}",
            escape_hook_point_segment(agent_id),
            escape_hook_point_segment(tool_name),
            stage
        ))
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
        let mut hookers = runtime.hookers().list_for_hook_point(&hook_point);
        hookers.retain(|hooker| runtime.hookers().is_enabled(hooker.id()));
        hookers.sort_by(|left, right| left.id().0.cmp(&right.id().0));

        // Same criterion as the pre-hook sequence: the escaped hook point is
        // always well formed, so an empty match means no enabled hooker is
        // attached to this tool phase and the phase is skipped.
        if hookers.is_empty() {
            return Ok(Vec::new());
        }

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
                    call: hooker_visible_call(&state.final_call),
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

            let post_result = match output.primary {
                HookInvokePrimary::Post(post_result) => post_result,
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
        let mut hookers = runtime.hookers().list_for_hook_point(&hook_point);
        hookers.retain(|hooker| runtime.hookers().is_enabled(hooker.id()));
        hookers.sort_by(|left, right| left.id().0.cmp(&right.id().0));

        // Same criterion as the pre-hook sequence: the escaped hook point is
        // always well formed, so an empty match means no enabled hooker is
        // attached to this tool phase and the phase is skipped.
        if hookers.is_empty() {
            return Ok(Vec::new());
        }

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
                    call: hooker_visible_call(&state.final_call),
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

            let error_result = match output.primary {
                HookInvokePrimary::Error(error_result) => error_result,
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

/// Inject the hooker id into a plugin-produced `extra` payload and append it
/// to the call's accumulated `extra` array. Shared by the allow and transform
/// pre-hook arms so both result kinds contribute data identically; the
/// accumulated entries stay invisible to subsequent plugin hookers via
/// [`hooker_visible_call`] and reach the execution backend via
/// `FinalToolCall.extra`. A `None` or `null` payload contributes nothing.
fn accumulate_extra(final_call: &mut FinalToolCall, hooker_id: &HookerId, extra: Option<Value>) {
    if let Some(mut entry) = extra.filter(|entry| !entry.is_null()) {
        if let Value::Object(map) = &mut entry {
            map.insert("hooker_id".to_string(), json!(hooker_id.to_string()));
        }
        match &mut final_call.extra {
            Some(Value::Array(entries)) => entries.push(entry),
            _ => {
                final_call.extra = Some(Value::Array(vec![entry]));
            }
        }
    }
}

/// The `call` handed to plugin hookers hides the framework's accumulated
/// `extra` channel: plugins must not observe each other's contributions.
/// Accumulation into `state.final_call.extra` is untouched, so the backend
/// still receives every entry.
fn hooker_visible_call(final_call: &FinalToolCall) -> FinalToolCall {
    let mut call = final_call.clone();
    call.extra = None;
    call
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/tool/framework/call/call_impl/hooks_test.rs"]
mod tests;
