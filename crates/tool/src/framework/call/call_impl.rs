use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolCall, ToolExecutor, ToolSpecView};
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_types::hooker::{HookInvokeInput, HookInvokeOutput, HookPointId};
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, FinalToolCall, PostHookResult, PostToolHookInput,
    PreHookResult, PreToolHookInput, RawToolOutcome, ToolExecutionError, ToolExecutionResult,
    ToolExecutorOutput, ToolLifecycleRecord, ToolLifecycleStatus,
};
use async_trait::async_trait;
use hooker::{resolve_hook_point_category, HookPointCategory};
use serde_json::json;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

struct ToolExecutionState {
    final_call: FinalToolCall,
    trace_span: Option<TraceSpanHandle>,
    lifecycle_record: Option<ToolLifecycleRecord>,
    pre_hook_results: Vec<PreHookResult>,
    post_hook_results: Vec<PostHookResult>,
    error_hook_results: Vec<ErrorHookResult>,
    raw_outcome: Option<RawToolOutcome>,
    execution_error: Option<ToolExecutionError>,
}

enum ExecutorPhaseResult {
    Completed(RawToolOutcome),
    Suspended(String),
    Failed(ToolExecutionError),
}

pub struct ToolCallImpl {
    final_call: FinalToolCall,
    spec: Arc<dyn ToolSpecView>,
    executor: Arc<dyn ToolExecutor>,
}

impl ToolCallImpl {
    pub fn new(
        final_call: FinalToolCall,
        spec: Arc<dyn ToolSpecView>,
        executor: Arc<dyn ToolExecutor>,
    ) -> Self {
        Self {
            final_call,
            spec,
            executor,
        }
    }

    fn initialize_execution_state(&self) -> ToolExecutionState {
        ToolExecutionState {
            final_call: self.final_call.clone(),
            trace_span: None,
            lifecycle_record: None,
            pre_hook_results: Vec::new(),
            post_hook_results: Vec::new(),
            error_hook_results: Vec::new(),
            raw_outcome: None,
            execution_error: None,
        }
    }

    async fn run_pre_hook_sequence(
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

            let input = HookInvokeInput::Pre(PreToolHookInput {
                call: state.final_call.clone(),
            });

            let output = match hooker.invoke(input, runtime).await {
                Ok(output) => output,
                Err(error) => {
                    eprintln!(
                        "pre-hook invoke failed for hooker '{}' on tool call '{}' (tool='{}'): {}",
                        hooker.id(),
                        state.final_call.call_id,
                        state.final_call.tool_name,
                        error
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
                    eprintln!(
                        "pre-hooker '{}' returned non-pre output {:?} for tool call '{}'",
                        hooker.id(),
                        other,
                        state.final_call.call_id
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

    fn build_tool_hook_point(
        &self,
        runtime: &dyn RuntimeView,
        tool_name: &str,
        stage: &str,
    ) -> HookPointId {
        let agent_id = &runtime.agent_context().metadata().agent_id;
        HookPointId(format!("{}.Tool.{}.{}", agent_id, tool_name, stage))
    }

    fn load_execution_context(
        &self,
        final_call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolLifecycleRecord, ToolExecutionError> {
        Ok(runtime.state_store().begin(final_call, self.spec.as_ref()))
    }

    fn mark_lifecycle_running(&self, state: &mut ToolExecutionState, runtime: &dyn RuntimeView) {
        if let Some(record) = state.lifecycle_record.as_mut() {
            record.status = ToolLifecycleStatus::Running;
            runtime.state_store().update(record);
        }
    }

    fn persist_terminal_lifecycle(
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

    async fn collect_error_hook_results_after_begin(
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
                eprintln!(
                    "error-hook phase failed for tool call '{}' (tool='{}') after begin: {}",
                    state.final_call.call_id, state.final_call.tool_name, error_hook_failure
                );
            }
        }
    }

    async fn invoke_resolved_executor(
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

    async fn run_post_hook_sequence(
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

            let input = HookInvokeInput::Post(PostToolHookInput {
                call: state.final_call.clone(),
                outcome: current_outcome,
            });

            let output = match hooker
                .invoke(input, runtime)
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed {
                    message: e.to_string(),
                }) {
                Ok(o) => o,
                Err(e) => {
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": e.to_string()}),
                        )
                        .await;
                    return Err(e);
                }
            };

            let post_result = match output {
                HookInvokeOutput::Post(post_result) => post_result,
                other => {
                    let err = ToolExecutionError::ExecutionFailed {
                        message: format!(
                            "post-hooker '{}' returned non-post output {:?} for tool call '{}'",
                            hooker.id(),
                            other,
                            state.final_call.call_id
                        ),
                    };
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": err.to_string()}),
                        )
                        .await;
                    return Err(err);
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

    async fn run_error_hook_sequence(
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

            let input = HookInvokeInput::Error(ErrorToolHookInput {
                call: state.final_call.clone(),
                error: execution_error.clone(),
            });

            let output = match hooker
                .invoke(input, runtime)
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed {
                    message: e.to_string(),
                }) {
                Ok(o) => o,
                Err(e) => {
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": e.to_string()}),
                        )
                        .await;
                    return Err(e);
                }
            };

            let error_result = match output {
                HookInvokeOutput::Error(error_result) => error_result,
                other => {
                    let err = ToolExecutionError::ExecutionFailed {
                        message: format!(
                            "error-hooker '{}' returned non-error output {:?} for tool call '{}'",
                            hooker.id(),
                            other,
                            state.final_call.call_id
                        ),
                    };
                    runtime
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": err.to_string()}),
                        )
                        .await;
                    return Err(err);
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

    fn build_denied_result(&self, state: &ToolExecutionState) -> ToolExecutionResult {
        ToolExecutionResult::Denied {
            final_call: state.final_call.clone(),
            pre_hook_results: state.pre_hook_results.clone(),
            error_hook_results: state.error_hook_results.clone(),
            error: state.execution_error.clone(),
        }
    }

    fn build_failed_result(
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

    fn build_completed_result(
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

    fn build_suspended_result(
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

    async fn begin_trace_span(&self, state: &mut ToolExecutionState, runtime: &dyn RuntimeView) {
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

    async fn update_trace_span(
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

    async fn end_trace_span(
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

#[async_trait]
impl ToolCall for ToolCallImpl {
    fn final_call(&self) -> &FinalToolCall {
        &self.final_call
    }

    async fn execute(
        &self,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutionResult, ToolExecutionError> {
        let mut state = self.initialize_execution_state();
        self.begin_trace_span(&mut state, runtime).await;

        match self.run_pre_hook_sequence(&mut state, runtime).await {
            Ok(pre_hook_results) => {
                state.pre_hook_results = pre_hook_results;
                self.update_trace_span(&state, runtime, "pre_hooks_done")
                    .await;
            }
            Err(error) => {
                eprintln!(
                    "pre-hook phase failed for tool call '{}' (tool='{}'): {}",
                    state.final_call.call_id, state.final_call.tool_name, error
                );
            }
        }

        if state
            .pre_hook_results
            .iter()
            .any(|result| matches!(result, PreHookResult::Deny { .. }))
        {
            let result = self.build_denied_result(&state);
            self.end_trace_span(
                &mut state,
                runtime,
                TraceOutcome::Denied,
                &result,
                "pre_hook_deny",
            )
            .await;
            return Ok(result);
        }

        match self.load_execution_context(&state.final_call, runtime) {
            Ok(lifecycle_record) => {
                state.lifecycle_record = Some(lifecycle_record);
                self.mark_lifecycle_running(&mut state, runtime);
                self.update_trace_span(&state, runtime, "running").await;
            }
            Err(error) => {
                state.execution_error = Some(error.clone());
                match self.run_error_hook_sequence(&state, &error, runtime).await {
                    Ok(error_hook_results) => {
                        state.error_hook_results = error_hook_results;
                    }
                    Err(error_hook_failure) => {
                        eprintln!(
                            "error-hook phase failed for tool call '{}' (tool='{}') after lifecycle begin failure: {}",
                            state.final_call.call_id,
                            state.final_call.tool_name,
                            error_hook_failure
                        );
                    }
                }
                let result = self.build_failed_result(&state, error);
                self.end_trace_span(
                    &mut state,
                    runtime,
                    TraceOutcome::Error,
                    &result,
                    "lifecycle_begin_failed",
                )
                .await;
                return Ok(result);
            }
        }

        match self
            .invoke_resolved_executor(&state.final_call, runtime)
            .await
        {
            Ok(ExecutorPhaseResult::Completed(raw_outcome)) => {
                state.raw_outcome = Some(raw_outcome);
                self.update_trace_span(&state, runtime, "executor_completed")
                    .await;
            }
            Ok(ExecutorPhaseResult::Suspended(suspend_token)) => {
                let result = self.build_suspended_result(&state, suspend_token);
                self.persist_terminal_lifecycle(&mut state, &result, runtime);
                self.end_trace_span(&mut state, runtime, TraceOutcome::Ok, &result, "suspended")
                    .await;
                return Ok(result);
            }
            Ok(ExecutorPhaseResult::Failed(execution_error)) => {
                state.execution_error = Some(execution_error);
                self.update_trace_span(&state, runtime, "executor_failed")
                    .await;
            }
            Err(error) => {
                state.execution_error = Some(error.clone());
                self.collect_error_hook_results_after_begin(&mut state, &error, runtime)
                    .await;
                self.update_trace_span(&state, runtime, "executor_invoke_error")
                    .await;
                let result = self.build_failed_result(&state, error);
                self.persist_terminal_lifecycle(&mut state, &result, runtime);
                self.end_trace_span(
                    &mut state,
                    runtime,
                    TraceOutcome::Error,
                    &result,
                    "executor_invoke_error",
                )
                .await;
                return Ok(result);
            }
        }

        if let Some(execution_error) = state.execution_error.as_ref() {
            let execution_error = execution_error.clone();
            self.collect_error_hook_results_after_begin(&mut state, &execution_error, runtime)
                .await;
            self.update_trace_span(&state, runtime, "error_hooks_done")
                .await;
            let result = self.build_failed_result(&state, execution_error);
            self.persist_terminal_lifecycle(&mut state, &result, runtime);
            self.end_trace_span(
                &mut state,
                runtime,
                TraceOutcome::Error,
                &result,
                "executor_error",
            )
            .await;
            return Ok(result);
        }

        if state.raw_outcome.is_some() {
            match self.run_post_hook_sequence(&mut state, runtime).await {
                Ok(post_hook_results) => {
                    state.post_hook_results = post_hook_results;
                    self.update_trace_span(&state, runtime, "post_hooks_done")
                        .await;
                }
                Err(error) => {
                    state.execution_error = Some(error.clone());
                    self.collect_error_hook_results_after_begin(&mut state, &error, runtime)
                        .await;
                    self.update_trace_span(&state, runtime, "post_hook_error")
                        .await;
                    let result = self.build_completed_result(
                        &state,
                        state
                            .raw_outcome
                            .clone()
                            .expect("post-hook error should preserve completed raw_outcome"),
                    );
                    self.persist_terminal_lifecycle(&mut state, &result, runtime);
                    self.end_trace_span(
                        &mut state,
                        runtime,
                        TraceOutcome::Error,
                        &result,
                        "post_hook_error",
                    )
                    .await;
                    return Ok(result);
                }
            }
        }

        if let Some(execution_error) = state.execution_error.as_ref() {
            let execution_error = execution_error.clone();
            self.collect_error_hook_results_after_begin(&mut state, &execution_error, runtime)
                .await;
            self.update_trace_span(&state, runtime, "terminal_error_hooks_done")
                .await;
            let result = self.build_failed_result(&state, execution_error);
            self.persist_terminal_lifecycle(&mut state, &result, runtime);
            self.end_trace_span(
                &mut state,
                runtime,
                TraceOutcome::Error,
                &result,
                "terminal_error",
            )
            .await;
            return Ok(result);
        }

        let result = self.build_completed_result(
            &state,
            state
                .raw_outcome
                .clone()
                .expect("completed tool call should have raw_outcome"),
        );
        self.persist_terminal_lifecycle(&mut state, &result, runtime);
        self.end_trace_span(&mut state, runtime, TraceOutcome::Ok, &result, "completed")
            .await;
        Ok(result)
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
