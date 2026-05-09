mod hooks;
mod lifecycle;
mod results;
mod state;
#[cfg(test)]
mod tests;
mod trace;

use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolCall, ToolExecutor, ToolSpecView};
use agent_contracts::trace::TraceOutcome;
use agent_types::events::ToolLifecycleEvent;
use agent_types::tool::{FinalToolCall, PreHookResult, ToolExecutionError, ToolExecutionResult};
use async_trait::async_trait;
use std::sync::Arc;

use self::results::{denied_reason, format_tool_args_preview, result_error_message};
use self::state::{ExecutorPhaseResult, ToolExecutionState};

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
            self.emit_tool_event(
                runtime,
                ToolLifecycleEvent::Denied {
                    call_id: state.final_call.call_id.clone(),
                    tool_name: state.final_call.tool_name.clone(),
                    reason: denied_reason(&state),
                    args_preview: format_tool_args_preview(&state.final_call),
                },
            );
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
                self.emit_tool_event(
                    runtime,
                    ToolLifecycleEvent::Running {
                        call_id: state.final_call.call_id.clone(),
                        tool_name: state.final_call.tool_name.clone(),
                        args_preview: format_tool_args_preview(&state.final_call),
                    },
                );
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
                self.emit_tool_event(
                    runtime,
                    ToolLifecycleEvent::Failed {
                        call_id: state.final_call.call_id.clone(),
                        tool_name: state.final_call.tool_name.clone(),
                        error: result_error_message(&result),
                        args_preview: format_tool_args_preview(&state.final_call),
                    },
                );
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
                self.emit_tool_event(
                    runtime,
                    ToolLifecycleEvent::Failed {
                        call_id: state.final_call.call_id.clone(),
                        tool_name: state.final_call.tool_name.clone(),
                        error: result_error_message(&result),
                        args_preview: format_tool_args_preview(&state.final_call),
                    },
                );
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
            self.emit_tool_event(
                runtime,
                ToolLifecycleEvent::Failed {
                    call_id: state.final_call.call_id.clone(),
                    tool_name: state.final_call.tool_name.clone(),
                    error: result_error_message(&result),
                    args_preview: format_tool_args_preview(&state.final_call),
                },
            );
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
                    self.emit_tool_event(
                        runtime,
                        ToolLifecycleEvent::Completed {
                            call_id: state.final_call.call_id.clone(),
                            tool_name: state.final_call.tool_name.clone(),
                            args_preview: format_tool_args_preview(&state.final_call),
                        },
                    );
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
            self.emit_tool_event(
                runtime,
                ToolLifecycleEvent::Failed {
                    call_id: state.final_call.call_id.clone(),
                    tool_name: state.final_call.tool_name.clone(),
                    error: result_error_message(&result),
                    args_preview: format_tool_args_preview(&state.final_call),
                },
            );
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
        self.emit_tool_event(
            runtime,
            ToolLifecycleEvent::Completed {
                call_id: state.final_call.call_id.clone(),
                tool_name: state.final_call.tool_name.clone(),
                args_preview: format_tool_args_preview(&state.final_call),
            },
        );
        self.end_trace_span(&mut state, runtime, TraceOutcome::Ok, &result, "completed")
            .await;
        Ok(result)
    }
}
