use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use agent_contracts::context::prompt::input::PromptBuildInput;
use agent_contracts::events::LoopEventSink;
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_contracts::{Hooker, RuntimeView};
use agent_llm::{AssistantMessageExt, ChatMessageExt, MessageRoleExt};
use agent_types::chat::{ChatSystemTransformInput, ChatSystemTransformResult, ModelRef};
use agent_types::compression::CompressedView;
use agent_types::context::prompt::result::PromptBuildResult;
use agent_types::hook::{
    HookInvokeInput, HookInvokeMetadata, HookInvokeOutput, HookInvokePrimary, HookPointId,
};
use agent_types::outcome::{AgentError, AgentOutcome};
use agent_types::tool::ToolExecutionResult;
use agent_types::{
    AgentId, AssistantMessage, ChatMessage, ContentBlock, LlmError, MessageRole, StopReason,
    StreamChunk, Usage,
};
use serde_json::{json, Value};

use crate::input::AgentLoopInput;
use crate::loop_state::LoopState;
use crate::runtime::AgentRuntime;
use crate::snapshot::RuntimeSnapshot;
use crate::suspend::LoopRunResult;
use crate::token_estimator::TokenEstimator;

use crate::spawn_evict;

#[path = "tool_exec.rs"]
mod tool_exec;

pub use tool_exec::build_tool_result_message;

pub enum LoopDecision {
    Continue,
    ReturnComplete,
    ReturnMaxTurns,
    ReturnBudgetExhausted,
    ReturnCancelled,
}

pub struct TurnState {
    pub turn_number: u32,
    pub compression_output: Option<CompressedView>,
    pub build_messages_output: Option<PromptBuildResult>,
    pub assistant_message: Option<AssistantMessage>,
    pub tool_results: Vec<ToolExecutionResult>,
    pub decision: Option<LoopDecision>,
    pub turn_span: Option<TraceSpanHandle>,
    pub ttft_ms: u64,
    pub total_time_ms: u64,
    pub tpot_ms: f64,
    pub force_return_complete: bool,
}

impl TurnState {
    pub fn new(turn_number: u32) -> Self {
        Self {
            turn_number,
            compression_output: None,
            build_messages_output: None,
            assistant_message: None,
            tool_results: Vec::new(),
            decision: None,
            turn_span: None,
            ttft_ms: 0,
            total_time_ms: 0,
            tpot_ms: 0.0,
            force_return_complete: false,
        }
    }
}

pub struct LoopContext<'a> {
    pub snapshot: RuntimeSnapshot,
    pub state: &'a mut LoopState,
    pub input: AgentLoopInput,
    pub turn: TurnState,
    /// Borrowed reference to the per-run token estimator so push sites can
    /// populate `ChatMessage::estimated_tokens` before appending. Without
    /// this, every pre-check / compress call re-estimates unchanged messages.
    pub estimator: &'a TokenEstimator,
}

pub async fn run_agent_loop(
    runtime: &AgentRuntime,
    state: &mut LoopState,
    mut input: AgentLoopInput,
) -> Result<LoopRunResult, AgentError> {
    let snapshot = runtime.snapshot();
    let estimator = TokenEstimator::new();

    // Detect `/skill-name` prefix and expand skill prompt inline.
    if input.append_user_message {
        if let Some(expanded) =
            try_expand_skill_prefix(&input.user_message, &*snapshot.skill_registry)
        {
            input.user_message = expanded;
        }
        // command.execute.before — fires before chat.message so the
        // command-layer body rewrite feeds into the message-layer hook.
        if let Some(cmd_ctx) = input.command_context.as_ref() {
            if let Some(runtime_view) = input.runtime_view.as_ref() {
                let (body, deny) = run_command_execute_before_hook(
                    runtime_view,
                    &state.session_id,
                    input.agent_id.as_ref(),
                    &cmd_ctx.command,
                    &cmd_ctx.arguments,
                    input.user_message.clone(),
                )
                .await;
                if let Some(reason) = deny {
                    let deny_text =
                        format!("Command '{}' denied by plugin: {}", cmd_ctx.command, reason);
                    let mut deny_msg =
                        ChatMessage::text(MessageRole::Assistant, &deny_text, now_ms());
                    estimator.update_message_cache(&mut deny_msg);
                    state.messages.write().push(deny_msg);
                    if let Some(ref sink) = input.event_sink {
                        let agent_id = agent_id_or_anonymous(input.agent_id.as_ref());
                        sink.on_assistant_message(agent_id, &deny_text);
                    }
                    return Ok(LoopRunResult::Complete(AgentOutcome::Complete {
                        reply: deny_text,
                        messages: state.messages.read().clone(),
                        turn_count: 0,
                        token_usage: agent_types::outcome::TokenUsage::default(),
                        estimated_input_tokens: 0,
                    }));
                }
                input.user_message = body;
            }
        }
        let candidate = ChatMessage::text(MessageRole::User, &input.user_message, now_ms());
        // chat.message — fires before the user message is persisted.
        let mut final_message = apply_chat_message_hook(
            input.runtime_view.as_ref(),
            &state.session_id.to_string(),
            input.agent_id.as_ref(),
            candidate,
        )
        .await;
        // Sync transformed text back into loop input for event sinks/tracing.
        // Non-text transforms are persisted as-is via the message push below.
        if let Some(text) = final_message.text_content() {
            input.user_message = text.to_string();
        }
        estimator.update_message_cache(&mut final_message);
        state.messages.write().push(final_message);
    }

    let mut ctx = LoopContext {
        snapshot,
        state,
        input,
        turn: TurnState::new(1),
        estimator: &estimator,
    };

    loop {
        ctx.turn = TurnState::new(ctx.turn.turn_number);
        begin_turn_span(&mut ctx).await;

        if let Some(ref sink) = ctx.input.event_sink {
            let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref());
            sink.on_turn_start(agent_id, ctx.turn.turn_number);
        }
        drain_pending_user_messages(&mut ctx).await;
        if let Err(error) = compress(&mut ctx, CompressionTrigger::Automatic).await {
            end_turn_span(
                &mut ctx,
                TraceOutcome::Error,
                json!({"stop_reason": "compression_error"}),
            )
            .await;
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Error,
                Some(error.to_string()),
                "compression_error",
            )
            .await;
            return Err(error);
        }
        if let Err(error) = build_messages(&mut ctx).await {
            end_turn_span(
                &mut ctx,
                TraceOutcome::Error,
                json!({"stop_reason": "prompt_build_error"}),
            )
            .await;
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Error,
                Some(error.to_string()),
                "prompt_build_error",
            )
            .await;
            return Err(error);
        }
        if let Err(error) = pre_check_token_budget(&mut ctx, &estimator).await {
            end_turn_span(
                &mut ctx,
                TraceOutcome::Error,
                json!({"stop_reason": "pre_check_error"}),
            )
            .await;
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Error,
                Some(error.to_string()),
                "pre_check_error",
            )
            .await;
            return Err(error);
        }
        if let Err(error) = llm_call_with_recovery(&mut ctx).await {
            end_turn_span(
                &mut ctx,
                TraceOutcome::Error,
                json!({"stop_reason": "llm_call_error"}),
            )
            .await;
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Error,
                Some(error.to_string()),
                "llm_call_error",
            )
            .await;
            return Err(error);
        }
        update_turn_span_after_llm(&mut ctx).await;
        let suspended_calls = match tool_exec::run(&mut ctx).await {
            Ok(suspended_calls) => suspended_calls,
            Err(error) => {
                end_turn_span(
                    &mut ctx,
                    TraceOutcome::Error,
                    json!({"stop_reason": "tool_exec_error"}),
                )
                .await;
                finalize_trace_for_ctx(
                    &ctx,
                    TraceOutcome::Error,
                    Some(error.to_string()),
                    "tool_exec_error",
                )
                .await;
                return Err(error);
            }
        };
        if !suspended_calls.is_empty() {
            end_turn_span(
                &mut ctx,
                TraceOutcome::Ok,
                json!({"stop_reason": "suspended"}),
            )
            .await;
            emit_loop_end(&ctx, "suspended");
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Ok,
                Some("suspended".to_string()),
                "suspended",
            )
            .await;
            return Ok(LoopRunResult::Suspended(suspended_calls));
        }
        decide(&mut ctx);

        match ctx.turn.decision {
            Some(LoopDecision::Continue) => {
                end_turn_span(
                    &mut ctx,
                    TraceOutcome::Ok,
                    json!({"stop_reason": "continue"}),
                )
                .await;
                ctx.state.turn_count += 1;
                ctx.turn = TurnState::new(ctx.turn.turn_number + 1);
            }
            Some(LoopDecision::ReturnComplete) => {
                ctx.state.turn_count += 1;
                break;
            }
            Some(LoopDecision::ReturnMaxTurns) => {
                ctx.state.turn_count += 1;
                let outcome = build_outcome_max_turns(&ctx);
                end_turn_span(
                    &mut ctx,
                    TraceOutcome::Error,
                    json!({"stop_reason": "max_turns"}),
                )
                .await;
                finalize_trace_for_ctx(
                    &ctx,
                    TraceOutcome::Error,
                    Some("max turns reached".to_string()),
                    "max_turns",
                )
                .await;
                emit_loop_end(&ctx, "max_turns");
                return Ok(LoopRunResult::Complete(outcome));
            }
            Some(LoopDecision::ReturnBudgetExhausted) => {
                ctx.state.turn_count += 1;
                let outcome = build_outcome_budget(&ctx);
                end_turn_span(
                    &mut ctx,
                    TraceOutcome::Error,
                    json!({"stop_reason": "budget_exhausted"}),
                )
                .await;
                finalize_trace_for_ctx(
                    &ctx,
                    TraceOutcome::Error,
                    Some("budget exhausted".to_string()),
                    "budget_exhausted",
                )
                .await;
                emit_loop_end(&ctx, "budget_exhausted");
                return Ok(LoopRunResult::Complete(outcome));
            }
            Some(LoopDecision::ReturnCancelled) => {
                let outcome = build_outcome_cancelled(&ctx);
                end_turn_span(
                    &mut ctx,
                    TraceOutcome::Cancelled,
                    json!({"stop_reason": "cancelled"}),
                )
                .await;
                finalize_trace_for_ctx(
                    &ctx,
                    TraceOutcome::Cancelled,
                    Some("cancelled".to_string()),
                    "cancelled",
                )
                .await;
                emit_loop_end(&ctx, "cancelled");
                return Ok(LoopRunResult::Complete(outcome));
            }
            None => {
                let error = AgentError::LlmProvider("loop decision was not set".into());
                end_turn_span(
                    &mut ctx,
                    TraceOutcome::Error,
                    json!({"stop_reason": "missing_decision"}),
                )
                .await;
                finalize_trace_for_ctx(
                    &ctx,
                    TraceOutcome::Error,
                    Some(error.to_string()),
                    "missing_decision",
                )
                .await;
                return Err(error);
            }
        }
    }
    end_turn_span(
        &mut ctx,
        TraceOutcome::Ok,
        json!({"stop_reason": "complete"}),
    )
    .await;

    let reply = ctx
        .turn
        .assistant_message
        .as_ref()
        .and_then(|m| m.text.clone())
        .unwrap_or_default();

    emit_loop_end(&ctx, "complete");

    finalize_trace_for_ctx(&ctx, TraceOutcome::Ok, None, "complete").await;

    // Extract the messages snapshot and token estimate to locals before
    // constructing the return value so the `RwLockReadGuard` temporary is
    // dropped before `ctx` (and therefore `estimator`) goes out of scope.
    let messages = ctx.state.messages.read().clone();
    let turn_count = ctx.state.turn_count;
    let token_usage = ctx.state.token_usage.clone();
    let estimated_input_tokens = current_turn_estimated_input_tokens(&ctx);

    Ok(LoopRunResult::Complete(AgentOutcome::Complete {
        reply,
        messages,
        turn_count,
        token_usage,
        estimated_input_tokens,
    }))
}

async fn drain_pending_user_messages(ctx: &mut LoopContext<'_>) {
    let Some(source) = ctx.input.pending_user_messages.clone() else {
        return;
    };

    for message in source.drain_pending_user_messages().await {
        if message.trim().is_empty() {
            continue;
        }
        let candidate = ChatMessage::text(MessageRole::User, &message, now_ms());
        let mut final_message = apply_chat_message_hook(
            ctx.input.runtime_view.as_ref(),
            &ctx.state.session_id,
            ctx.input.agent_id.as_ref(),
            candidate,
        )
        .await;
        ctx.estimator.update_message_cache(&mut final_message);
        ctx.state.messages.write().push(final_message);
    }
}

async fn finalize_trace_for_ctx(
    ctx: &LoopContext<'_>,
    outcome: TraceOutcome,
    message: Option<String>,
    stop_reason: &'static str,
) {
    let Some(runtime_view) = ctx.input.runtime_view.as_ref() else {
        return;
    };

    runtime_view
        .trace_recorder()
        .finalize_trace(
            outcome,
            json!({
                "message": message,
                "stop_reason": stop_reason,
                "turn_count": ctx.state.turn_count,
                "total_tokens": ctx.state.token_usage.total_tokens,
            }),
        )
        .await;
}

async fn begin_turn_span(ctx: &mut LoopContext<'_>) {
    let Some(runtime_view) = ctx.input.runtime_view.clone() else {
        return;
    };
    let agent_id = chat_agent_segment(ctx.input.agent_id.as_ref());
    let span = runtime_view
        .trace_recorder()
        .begin_span(
            TraceSpanKind::Turn,
            std::borrow::Cow::Borrowed("turn"),
            json!({
                "turn_number": ctx.turn.turn_number,
                "agent_id": agent_id,
            }),
        )
        .await;
    ctx.turn.turn_span = Some(span);
}

async fn update_turn_span_after_llm(ctx: &mut LoopContext<'_>) {
    let Some(runtime_view) = ctx.input.runtime_view.clone() else {
        return;
    };
    let Some(span) = ctx.turn.turn_span.as_ref() else {
        return;
    };
    let (prompt_tokens, completion_tokens, total_tokens, cached_tokens, has_tool_calls) =
        match ctx.turn.assistant_message.as_ref() {
            Some(msg) => (
                msg.usage.prompt_tokens,
                msg.usage.completion_tokens,
                msg.usage.total_tokens,
                msg.usage.cached_tokens,
                msg.has_tool_calls(),
            ),
            None => (0, 0, 0, 0, false),
        };
    runtime_view
        .trace_recorder()
        .update_span(
            span,
            json!({
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": total_tokens,
                "cached_tokens": cached_tokens,
                "has_tool_calls": has_tool_calls,
            }),
        )
        .await;
}

async fn end_turn_span(
    ctx: &mut LoopContext<'_>,
    outcome: TraceOutcome,
    fields: serde_json::Value,
) {
    let Some(runtime_view) = ctx.input.runtime_view.clone() else {
        return;
    };
    let Some(span) = ctx.turn.turn_span.take() else {
        return;
    };
    runtime_view
        .trace_recorder()
        .end_span(span, outcome, fields)
        .await;
}

#[derive(Clone, Copy)]
enum CompressionTrigger {
    Automatic,
    ContextLimitRetry,
    PreCheckExceeded,
}

impl CompressionTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::ContextLimitRetry => "context_limit_retry",
            Self::PreCheckExceeded => "pre_check_exceeded",
        }
    }

    fn is_forced(self) -> bool {
        matches!(self, Self::ContextLimitRetry | Self::PreCheckExceeded)
    }
}

async fn pre_check_token_budget(
    ctx: &mut LoopContext<'_>,
    estimator: &TokenEstimator,
) -> Result<(), AgentError> {
    let estimated_input = ctx.state.with_messages(|messages| {
        estimator.estimate_input_tokens(
            &ctx.snapshot.system_prompt,
            ctx.snapshot.tool_registry.spec_count(),
            messages,
        )
    });

    let context_window = ctx.snapshot.token_budget_config.total_budget;
    let max_tokens = ctx.snapshot.token_budget_config.reserved_for_output;
    let reserved_for_system = ctx.snapshot.token_budget_config.reserved_for_system;
    let available_for_input = context_window
        .saturating_sub(max_tokens)
        .saturating_sub(reserved_for_system);

    if estimated_input <= available_for_input {
        tracing::debug!(
            estimated_input_tokens = estimated_input,
            available_tokens = available_for_input,
            context_window,
            "Pre-check passed: input tokens within budget"
        );
        return Ok(());
    }

    tracing::warn!(
        estimated_input_tokens = estimated_input,
        available_tokens = available_for_input,
        context_window,
        max_tokens,
        trigger = "pre_check_exceeded",
        "Pre-check failed: input tokens exceed available budget, triggering compression"
    );

    compress(ctx, CompressionTrigger::PreCheckExceeded).await?;
    build_messages(ctx).await?;

    let new_estimated = ctx.state.with_messages(|new_messages| {
        estimator.estimate_input_tokens(
            &ctx.snapshot.system_prompt,
            ctx.snapshot.tool_registry.spec_count(),
            new_messages,
        )
    });

    if new_estimated <= available_for_input {
        tracing::info!(
            new_estimated_tokens = new_estimated,
            available_tokens = available_for_input,
            "Pre-check passed after compression"
        );
        return Ok(());
    }

    if available_for_input == 0 || max_tokens + reserved_for_system >= context_window {
        tracing::warn!(
            estimated_tokens = new_estimated,
            available_tokens = available_for_input,
            context_window,
            max_tokens,
            reserved_for_system,
            "Pre-check detected invalid configuration: no available input space. \
             System will attempt API call to trigger auto-detection of actual context window."
        );
        return Ok(()); // Allow API call to proceed for auto-detection
    }

    tracing::warn!(
        estimated_tokens = new_estimated,
        available_tokens = available_for_input,
        context_window,
        "Pre-check shows input exceeds budget, but allowing API call for potential auto-adjustment"
    );

    Ok(())
}

async fn compress(
    ctx: &mut LoopContext<'_>,
    trigger: CompressionTrigger,
) -> Result<(), AgentError> {
    // Cheap, model-agnostic per-turn cleanup before any LLM-backed compaction.
    // microcompact drops stale tool_use/tool_result pairs from completed turns
    // (no API call), so it is the first line of defense against context bloat:
    // it keeps history lean between full compression events, which is exactly
    // what stops the expensive LLM collapse from firing every turn on active
    // sessions. Its 120s staleness + tail-window protection make it a no-op
    // during rapid turns, so history stays effectively append-only here.
    let now_ms = now_ms();
    {
        let messages = ctx.state.messages.read().clone();
        let micro = ctx
            .snapshot
            .compression_pipeline
            .microcompact(&messages, now_ms);
        if micro.applied {
            tracing::info!(
                removed = micro.removed_count,
                token_delta = micro.token_delta,
                removed_call_ids = ?micro.removed_call_ids,
                "microcompact: pruned stale tool_use/tool_result pairs"
            );
            let mut new_messages = micro.messages;
            for msg in &mut new_messages {
                ctx.estimator.update_message_cache(msg);
            }
            *ctx.state.messages.write() = new_messages;
        }
    }

    let agent_id_str = ctx
        .input
        .agent_id
        .as_ref()
        .map(|id| id.0.clone())
        .unwrap_or_default();

    // begin span — record baseline metadata at start
    let compression_span = if let Some(rv) = ctx.input.runtime_view.clone() {
        Some(
            rv.trace_recorder()
                .begin_span(
                    TraceSpanKind::Compression,
                    std::borrow::Cow::Borrowed("compression"),
                    json!({
                        "turn_number": ctx.turn.turn_number,
                        "agent_id": agent_id_str,
                        "message_count": ctx.state.messages.read().len(),
                        "trigger": trigger.as_str(),
                    }),
                )
                .await,
        )
    } else {
        None
    };

    // Analyze under the read guard — `analyze` is sync and borrows
    // `&[ChatMessage]`; release before any subsequent `.await`.
    let (analysis, msg_count_for_log) = ctx.state.with_messages(|messages| {
        let analysis = ctx
            .snapshot
            .compression_pipeline
            .analyze(messages, &*ctx.snapshot.token_budget_policy);
        (analysis, messages.len())
    });

    tracing::debug!(
        estimated = analysis.estimated_tokens,
        available = analysis.available_tokens,
        ratio = format!("{:.1}%", analysis.usage_ratio * 100.0),
        severity = ?analysis.severity,
        msg_count = msg_count_for_log,
        "compression analysis"
    );

    // update span — record analysis results
    if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span.as_ref()) {
        rv.trace_recorder()
            .update_span(
                span,
                json!({
                    "estimated_tokens": analysis.estimated_tokens,
                    "available_tokens": analysis.available_tokens,
                    "usage_ratio": analysis.usage_ratio,
                    "severity": format!("{:?}", analysis.severity),
                    "needs_compression": analysis.needs_compression(),
                    "forced": trigger.is_forced(),
                }),
            )
            .await;
    }

    if !trigger.is_forced() && !analysis.needs_compression() {
        // end span — no compression needed, normal end
        if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span) {
            rv.trace_recorder()
                .end_span(span, TraceOutcome::Ok, json!({ "skipped": true }))
                .await;
        }
        return Ok(());
    }

    let msg_count_before = ctx.state.messages.read().len();

    // Clone messages before .await to avoid holding RwLockReadGuard across await point
    let messages = ctx.state.messages.read().clone();
    let view = ctx
        .snapshot
        .compression_pipeline
        .compress(
            &messages,
            &*ctx.snapshot.token_budget_policy,
            &ctx.state.compression_meta,
        )
        .await
        .map_err(|e| AgentError::Compression(e.to_string()));

    match view {
        Ok(mut view) => {
            // Prune stale tool output as part of the compression event and
            // persist the result. Compression already invalidates the
            // provider prefix cache wholesale (history is rewritten), so
            // piggybacking the prune here is free — whereas pruning per turn
            // would move the prune frontier every turn and permanently cap
            // cache hits at it. Between compressions history stays strictly
            // append-only. Note: the summarizer above saw the full outputs;
            // only the retained messages are pruned.
            prune_stale_tool_output(&mut view.messages);

            tracing::info!(
                severity = ?analysis.severity,
                usage_ratio = format!("{:.1}%", analysis.usage_ratio * 100.0),
                estimated_tokens = analysis.estimated_tokens,
                messages_before = msg_count_before,
                messages_after = view.messages.len(),
                removed = view.removed_count,
                has_summary = view.summary.is_some(),
                trigger = trigger.as_str(),
                "context compression triggered"
            );

            // end span — compression succeeded, record output info
            if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span) {
                rv.trace_recorder()
                    .end_span(
                        span,
                        TraceOutcome::Ok,
                        json!({
                            "skipped": false,
                            "forced": trigger.is_forced(),
                            "messages_before": msg_count_before,
                            "messages_after": view.messages.len(),
                            "removed_count": view.removed_count,
                            "has_summary": view.summary.is_some(),
                            "estimated_tokens_after": view.estimated_tokens,
                        }),
                    )
                    .await;
            }

            // Compression may synthesize/trim messages; the returned
            // `view.messages` is a fresh Vec without per-message token
            // caches. Populate them so the next analyze reads cached values.
            let mut new_messages = view.messages.clone();
            for msg in &mut new_messages {
                ctx.estimator.update_message_cache(msg);
            }
            *ctx.state.messages.write() = new_messages;
            ctx.state.compression_meta = view.updated_meta.clone();
            ctx.turn.compression_output = Some(view);

            Ok(())
        }
        Err(e) => {
            // end span — compression failed, record error info
            if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span) {
                rv.trace_recorder()
                    .end_span(span, TraceOutcome::Error, json!({ "error": e.to_string() }))
                    .await;
            }
            Err(e)
        }
    }
}

fn prune_stale_tool_output(messages: &mut [ChatMessage]) {
    const KEEP_RECENT_TOOL_BYTES: usize = 40_000;
    const MIN_PRUNABLE_BYTES: usize = 1_000;
    const PRUNED_MARKER: &str =
        "[older tool output pruned to save context — re-run the tool or read the file if you still need it]";
    let mut kept = 0usize;
    let mut pruned = 0usize;
    for message in messages.iter_mut().rev() {
        for block in message.blocks.iter_mut() {
            if let ContentBlock::ToolResult { output, .. } = block {
                if output.as_str() == PRUNED_MARKER {
                    continue;
                }
                if kept < KEEP_RECENT_TOOL_BYTES {
                    kept += output.len();
                } else if output.len() > MIN_PRUNABLE_BYTES {
                    *output = PRUNED_MARKER.to_string();
                    pruned += 1;
                }
            }
        }
    }
    if pruned > 0 {
        tracing::debug!(pruned, "pruned stale tool output beyond recent window");
    }
}

/// Per-turn dynamic context: the remaining horizon and the live `todo_write`
/// plan. Rendered by the prompt builder into an ephemeral `<system-reminder>`
/// message appended at the END of each request (see
/// `prompt::compose::compose_turn_context_reminder`) — never into the system
/// prompt, whose per-turn churn would break provider prefix caching for the
/// entire conversation history behind it.
fn live_context_snippets(
    ctx: &LoopContext<'_>,
) -> Vec<agent_types::context::prompt::MemorySnippet> {
    use agent_types::context::prompt::MemorySnippet;
    let mut snippets = Vec::new();

    let turn = ctx.turn.turn_number;
    let max_turns = ctx.snapshot.max_turns;
    let tokens_used = ctx.state.token_usage.total_tokens;
    let remaining = max_turns.saturating_sub(turn);
    if max_turns > 0 && remaining <= 5 {
        let horizon = format!(
            "- turn: {turn}/{max_turns} ({remaining} remaining)\n- tokens used so far: ~{tokens_used}\n- NEARING THE TURN LIMIT — stop investigating and converge now: apply your best fix, save the files, and finish this turn. A committed partial fix beats an unfinished exploration that gets cut off."
        );
        snippets.push(MemorySnippet {
            source: "horizon".to_string(),
            content: horizon,
            relevance_score: 1.0,
        });
    }

    let window = ctx.snapshot.token_budget_config.total_budget;
    let context_input = ctx.state.token_usage.prompt_tokens;
    if window > 0 {
        let pct = context_input.saturating_mul(100) / window;
        if pct >= 25 {
            let mut line =
                format!("- context window: ~{pct}% used ({context_input}/{window} input tokens)");
            if pct >= 75 {
                line.push_str(
                    " — running full; converge and finish before the earliest context is compacted away.",
                );
            }
            snippets.push(MemorySnippet {
                source: "budget".to_string(),
                content: line,
                relevance_score: 0.95,
            });
        }
    }

    // Active plan: open `todo_write` items for this session, re-injected every
    // turn so plan state is load-bearing rather than write-only.
    if let Some(runtime_view) = ctx.input.runtime_view.as_ref() {
        for line in tool::open_todo_lines(runtime_view.as_ref()) {
            snippets.push(MemorySnippet {
                source: "plan".to_string(),
                content: line,
                relevance_score: 0.9,
            });
        }
    }

    snippets
}

async fn build_messages(ctx: &mut LoopContext<'_>) -> Result<(), AgentError> {
    let skill_summaries = ctx.snapshot.skill_registry.list_skills();

    let agent_id_str = ctx
        .input
        .agent_id
        .as_ref()
        .map(|id| id.0.clone())
        .unwrap_or_default();

    // begin span — record baseline metadata at start
    let prompt_build_span = if let Some(rv) = ctx.input.runtime_view.clone() {
        Some(
            rv.trace_recorder()
                .begin_span(
                    TraceSpanKind::PromptBuild,
                    std::borrow::Cow::Borrowed("prompt_build"),
                    json!({
                        "turn_number": ctx.turn.turn_number,
                        "agent_id": agent_id_str,
                    }),
                )
                .await,
        )
    } else {
        None
    };

    let is_final_turn =
        ctx.snapshot.max_turns > 0 && ctx.turn.turn_number >= ctx.snapshot.max_turns;
    let visible_tools = if is_final_turn {
        Vec::new()
    } else {
        ctx.input.visible_tools.clone()
    };

    // History is projected as-is: append-only between compressions so every
    // request shares a byte-identical prefix with the previous one (provider
    // prefix caching). Stale tool output is pruned only inside `compress`,
    // where the cache is being invalidated wholesale anyway.
    let projected_messages = ctx.state.messages.read().clone();

    let input = PromptBuildInput {
        system_prompt: ctx.snapshot.system_prompt.to_string(),
        messages: projected_messages,
        visible_tools,
        skill_summaries,
        memory_snippets: live_context_snippets(ctx),
        environment: agent_types::context::prompt::EnvironmentInfo {
            model: String::new(),
            cwd: String::new(),
            workspace_root: None,
            date: String::new(),
            agent_id: agent_id_str,
        },
        feature_flags: ctx.snapshot.feature_flags.clone(),
        turn_count: ctx.turn.turn_number,
        budget: ctx.snapshot.token_budget_config.clone(),
    };

    // update span — record input dimension info after build completion
    if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), prompt_build_span.as_ref()) {
        rv.trace_recorder()
            .update_span(
                span,
                json!({
                    "message_count": input.messages.len(),
                    "visible_tool_count": input.visible_tools.len(),
                    "skill_count": input.skill_summaries.len(),
                    "has_system_prompt": !input.system_prompt.is_empty(),
                }),
            )
            .await;
    }

    let result = ctx
        .snapshot
        .prompt_builder
        .build(input)
        .await
        .map_err(|e| AgentError::PromptBuild(e.to_string()));

    match result {
        Ok(mut result) => {
            result.request.reasoning_effort = ctx.input.reasoning_effort;

            // system.transform — fires on un-merged system parts.
            run_chat_system_transform_sequence(ctx, &mut result).await;

            // end span — success, record estimated token count and other output info
            if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), prompt_build_span) {
                rv.trace_recorder()
                    .end_span(
                        span,
                        TraceOutcome::Ok,
                        json!({
                            "estimated_input_tokens": result.estimated_input_tokens,
                            "request_message_count": result.request.messages.len(),
                            "reasoning_effort": result.request.reasoning_effort.to_string(),
                        }),
                    )
                    .await;
            }
            ctx.turn.build_messages_output = Some(result);
            Ok(())
        }
        Err(e) => {
            // end span — failure, record error info
            if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), prompt_build_span) {
                rv.trace_recorder()
                    .end_span(
                        span,
                        TraceOutcome::Error,
                        json!({
                            "error": e.to_string(),
                        }),
                    )
                    .await;
            }
            Err(e)
        }
    }
}

/// Per-hooker application outcome returned by the callback driving
/// [`run_chat_hook_chain`].
enum HookApply {
    /// Apply succeeded; record `span_fields` on the trace span and keep
    /// iterating the remaining hookers.
    Continue(Value),
    /// Apply succeeded and wants the chain to stop early (e.g. a `Deny`
    /// result). Records `span_fields` then breaks the loop.
    Break(Value),
}

/// Collects the hookers registered for `hook_point`, keeping only enabled
/// ones and sorting by id for a stable, predictable execution order.
fn enabled_hookers_for<'a>(
    runtime_view: &'a Arc<dyn RuntimeView>,
    hook_point: &HookPointId,
) -> Vec<&'a dyn Hooker> {
    let mut hookers = runtime_view.hookers().list_for_hook_point(hook_point);
    hookers.retain(|h| runtime_view.hookers().is_enabled(h.id()));
    hookers.sort_by(|a, b| a.id().0.cmp(&b.id().0));
    hookers
}

/// Extracts the inner payload of a [`HookInvokeOutput`] primary variant, or
/// returns an error string naming the expected variant when the output's
/// primary is any other variant. Used inside `apply` closures to keep the
/// downcast terse. Actions on the output are ignored here; the dispatcher
/// drains them separately.
macro_rules! downcast_hook_output {
    ($output:expr, $variant:ident) => {
        match $output.primary {
            HookInvokePrimary::$variant(r) => r,
            other => {
                return Err(format!(
                    "expected {} primary, got {other:?}",
                    stringify!($variant)
                ));
            }
        }
    };
}

/// Resolve the agent segment used to build chat-level hook point ids.
/// Falls back to `"anonymous"` when the loop has no agent id, mirroring
/// the convention used by the event-sink agent id resolution below.
fn chat_agent_segment(agent_id: Option<&AgentId>) -> String {
    agent_id
        .map(|id| id.0.clone())
        .unwrap_or_else(|| "anonymous".to_string())
}

/// Lazily-initialized shared `"anonymous"` agent id, used as the fallback by
/// [`agent_id_or_anonymous`] so the event-sink sites don't allocate a fresh
/// `AgentId` on every call.
static ANON_AGENT_ID: std::sync::OnceLock<AgentId> = std::sync::OnceLock::new();

/// Returns the loop's agent id, or a shared `"anonymous"` fallback when none
/// is configured. Consolidates the 7× `default_agent_id + unwrap_or` pattern
/// previously inlined at every event-sink emission site.
fn agent_id_or_anonymous(agent_id: Option<&AgentId>) -> &AgentId {
    agent_id.unwrap_or_else(|| ANON_AGENT_ID.get_or_init(|| AgentId("anonymous".to_string())))
}

/// Drives the common dispatch loop shared by the three chat-level hook
/// points. `build_input` produces the per-iteration [`HookInvokeInput`]
/// from the current accumulator; `apply` destructures the hooker's output,
/// mutates the accumulator, and returns either [`HookApply::Continue`] to
/// keep iterating or [`HookApply::Break`] to short-circuit the chain. A
/// hooker whose output variant does not match the hook point, or whose
/// invocation errors, is logged and skipped so a single bad hooker can't
/// break the turn.
async fn run_chat_hook_chain<Acc>(
    runtime_view: &Arc<dyn RuntimeView>,
    hook_point: HookPointId,
    span_name: &'static str,
    hook_kind: &'static str,
    acc: &mut Acc,
    build_input: impl Fn(&Acc) -> HookInvokeInput,
    mut apply: impl FnMut(&mut Acc, HookInvokeOutput) -> Result<HookApply, String>,
) {
    let hookers = enabled_hookers_for(runtime_view, &hook_point);
    if hookers.is_empty() {
        return;
    }

    for hooker in hookers {
        let hook_span = runtime_view
            .trace_recorder()
            .begin_span(
                TraceSpanKind::Hook,
                std::borrow::Cow::Borrowed(span_name),
                json!({
                    "hook_kind": hook_kind,
                    "hooker_id": hooker.id().to_string(),
                    "hook_point": hook_point.0,
                }),
            )
            .await;

        let input = build_input(acc);
        let output = match hooker.invoke(input, runtime_view.as_ref()).await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(
                    "{hook_kind} hook invoke failed for hooker '{}' (hook_point='{}'): {e}",
                    hooker.id(),
                    hook_point.0
                );
                runtime_view
                    .trace_recorder()
                    .end_span(
                        hook_span,
                        TraceOutcome::Error,
                        json!({"error": e.to_string()}),
                    )
                    .await;
                continue;
            }
        };

        let (span_fields, do_break) = match apply(acc, output) {
            Ok(HookApply::Continue(span_fields)) => (span_fields, false),
            Ok(HookApply::Break(span_fields)) => (span_fields, true),
            Err(err) => {
                tracing::warn!(
                    "{hook_kind} hooker '{}' returned unexpected output for hook_point '{}': {err}",
                    hooker.id(),
                    hook_point.0
                );
                runtime_view
                    .trace_recorder()
                    .end_span(hook_span, TraceOutcome::Error, json!({"error": err}))
                    .await;
                continue;
            }
        };
        runtime_view
            .trace_recorder()
            .end_span(hook_span, TraceOutcome::Ok, span_fields)
            .await;
        if do_break {
            break;
        }
    }
}

/// `*.Chat.system.transform` — fires on the prompt builder's un-merged
/// `system: Vec<String>` parts; a `Transform` result replaces both the
/// parts and the joined system message in `request.messages[0]`.
async fn run_chat_system_transform_sequence(ctx: &LoopContext<'_>, result: &mut PromptBuildResult) {
    let Some(runtime_view) = ctx.input.runtime_view.as_ref() else {
        return;
    };

    let agent_segment = chat_agent_segment(ctx.input.agent_id.as_ref());
    let hook_point = HookPointId(format!("{}.Chat.system.transform", agent_segment));

    let session_id = ctx.state.session_id.clone();
    // xiaoo's RuntimeSnapshot does not currently expose provider/model ids
    // at the loop level; pass an empty ModelRef so plugins that key on
    // model still receive the field. Populate when model metadata lands.
    let model = ModelRef::default();

    run_chat_hook_chain(
        runtime_view,
        hook_point,
        "chat_system_transform_hook",
        "chat_system_transform",
        result,
        |result| HookInvokeInput::ChatSystemTransform {
            input: ChatSystemTransformInput {
                session_id: Some(session_id.clone()),
                model: model.clone(),
                current_system: result.system_parts.clone(),
            },
            metadata: HookInvokeMetadata::default(),
        },
        |result, output| {
            let transform_result = downcast_hook_output!(output, ChatSystemTransform);
            match transform_result {
                ChatSystemTransformResult::Allow => {
                    Ok(HookApply::Continue(json!({"result": "allow"})))
                }
                ChatSystemTransformResult::Transform { system } => {
                    result.system_parts = system;
                    // Rewrite the merged system message in-place so the LlmRequest
                    // carries the plugin-authored system text.
                    let joined = result.system_parts.join("\n\n");
                    if let Some(first) = result.request.messages.first_mut() {
                        if first.role == MessageRole::System {
                            first.blocks.clear();
                            first.blocks.push(ContentBlock::Text { text: joined });
                        }
                    }
                    Ok(HookApply::Continue(json!({"result": "transform"})))
                }
            }
        },
    )
    .await;
}

/// `*.Chat.message.received` — fires before a user message is persisted.
/// Returns the (possibly transformed) message; on error or when no hookers
/// are configured, the original candidate is returned unchanged.
async fn run_chat_message_hook(
    runtime_view: &Arc<dyn RuntimeView>,
    session_id: &str,
    agent_id: Option<&AgentId>,
    candidate: ChatMessage,
) -> ChatMessage {
    let agent_segment = chat_agent_segment(agent_id);
    let hook_point = HookPointId(format!("{}.Chat.message.received", agent_segment));

    // Snapshot before the agent loop pushes the current user message into
    // the shared conversation storage — count reflects prior messages only.
    let prior_message_count = runtime_view.agent_context().conversation().message_count();

    let mut current = candidate;
    run_chat_hook_chain(
        runtime_view,
        hook_point,
        "chat_message_hook",
        "chat_message",
        &mut current,
        |message| HookInvokeInput::ChatMessage {
            input: agent_types::chat::ChatMessageHookInput {
                session_id: session_id.to_string(),
                agent: Some(agent_segment.clone()),
                model: None,
                message_id: message.message_id.clone(),
                message: message.clone(),
                prior_message_count,
            },
            metadata: HookInvokeMetadata::default(),
        },
        |message, output| {
            let result = downcast_hook_output!(output, ChatMessage);
            match result {
                agent_types::chat::ChatMessageHookResult::Accept => {
                    Ok(HookApply::Continue(json!({"result": "accept"})))
                }
                agent_types::chat::ChatMessageHookResult::Transform {
                    message: new_message,
                } => {
                    *message = new_message;
                    Ok(HookApply::Continue(json!({"result": "transform"})))
                }
            }
        },
    )
    .await;
    current
}

/// Run `*.Chat.message.received` over `candidate` when a runtime view is
/// configured, otherwise return `candidate` unchanged. Consolidates the
/// "optional hook + push" pattern shared between [`run_agent_loop`] and
/// [`drain_pending_user_messages`]; callers that need to sync the
/// transformed text back into their input read it off the returned message.
async fn apply_chat_message_hook(
    runtime_view: Option<&Arc<dyn RuntimeView>>,
    session_id: &str,
    agent_id: Option<&AgentId>,
    candidate: ChatMessage,
) -> ChatMessage {
    match runtime_view {
        Some(runtime_view) => {
            run_chat_message_hook(runtime_view, session_id, agent_id, candidate).await
        }
        None => candidate,
    }
}

/// `*.Chat.command.before` — fires on an expanded slash-command body
/// before it becomes the user message. Returns `(body, Option<deny_reason>)`;
/// `Some(reason)` short-circuits the turn.
async fn run_command_execute_before_hook(
    runtime_view: &Arc<dyn RuntimeView>,
    session_id: &str,
    agent_id: Option<&AgentId>,
    command: &str,
    arguments: &str,
    body: String,
) -> (String, Option<String>) {
    let agent_segment = chat_agent_segment(agent_id);
    let hook_point = HookPointId(format!("{}.Chat.command.before", agent_segment));

    let mut acc = (body, None);
    run_chat_hook_chain(
        runtime_view,
        hook_point,
        "chat_command_before_hook",
        "chat_command_before",
        &mut acc,
        |acc| HookInvokeInput::CommandExecuteBefore {
            input: agent_types::chat::CommandExecuteBeforeInput {
                command: command.to_string(),
                session_id: session_id.to_string(),
                arguments: arguments.to_string(),
                body: acc.0.clone(),
            },
            metadata: HookInvokeMetadata::default(),
        },
        |acc, output| {
            let result = downcast_hook_output!(output, CommandExecuteBefore);
            match result {
                agent_types::chat::CommandExecuteBeforeResult::Allow => {
                    Ok(HookApply::Continue(json!({"result": "allow"})))
                }
                agent_types::chat::CommandExecuteBeforeResult::Transform { body } => {
                    acc.0 = body;
                    Ok(HookApply::Continue(json!({"result": "transform"})))
                }
                agent_types::chat::CommandExecuteBeforeResult::Deny { reason } => {
                    acc.1 = Some(reason.clone());
                    Ok(HookApply::Break(
                        json!({"result": "deny", "reason": reason}),
                    ))
                }
            }
        },
    )
    .await;
    acc
}

async fn llm_call(ctx: &mut LoopContext<'_>) -> Result<(), LlmError> {
    if ctx.state.cancel.is_cancelled() {
        return Ok(());
    }

    let build_result = ctx
        .turn
        .build_messages_output
        .as_ref()
        .expect("build_messages must run before llm_call");

    let start = std::time::Instant::now();
    let first_token_at = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let event_sink = ctx.input.event_sink.clone();
    let streamed_text = Mutex::new(String::new());
    let streamed_reasoning = Mutex::new(String::new());
    // Throttle state for in-stream sink emits (last_emit_len, last_emit_at).
    // Without throttling, every chunk forces an O(history) clone + filter,
    // making streaming O(N²) in the response length.
    let last_text_emit: Mutex<StreamEmitState> = Mutex::new(StreamEmitState::default());
    let last_reasoning_emit: Mutex<StreamEmitState> = Mutex::new(StreamEmitState::default());

    // Extract secrets under the read guard — no clone of the whole message
    // history is needed. Redaction is display-only (history/snapshots hold
    // raw secrets anyway); when the flag is off (local TUI default), pass an
    // empty slice so the delta fast path streams unfiltered and the snapshot
    // fallback's filter is a no-op. The daemon inherits
    // `redact_secrets_display = true` from `FeatureFlags::default()`, keeping
    // its SSE path always-redacted.
    let all_secrets = ctx
        .state
        .with_messages(|messages| extract_secrets_from_messages(messages));
    let secrets: Vec<String> = if ctx.snapshot.feature_flags.redact_secrets_display {
        all_secrets
    } else {
        Vec::new()
    };

    let runtime_view = ctx.input.runtime_view.as_deref();
    // Clone the agent_id once for the whole stream (it doesn't change mid-stream).
    let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref()).clone();
    let response = if std::env::var("XIAOO_NON_STREAMING").is_ok() {
        ctx.snapshot
            .llm_provider
            .complete_scoped(runtime_view, &build_result.request)
            .await?
    } else {
        let first_token_at = std::sync::Arc::clone(&first_token_at);
        let on_chunk = |chunk: StreamChunk| {
            if first_token_at.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                first_token_at.store(
                    start.elapsed().as_millis() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            stream_assistant_chunk(
                event_sink.as_deref(),
                &agent_id,
                &streamed_text,
                &streamed_reasoning,
                &last_text_emit,
                &last_reasoning_emit,
                chunk,
                &secrets,
            );
        };
        // Race the in-flight stream against cancellation so an Esc lands
        // in milliseconds instead of after the model finishes the whole
        // response: the session supervisor persists the partial loop state
        // only once this call returns (and the TUI's `/save` /
        // interrupt auto-save wait on that — see the endside drain fix).
        // The race lives inside the provider wrapper
        // (`complete_stream_scoped_with_cancel`) so the wrapper can close
        // its LLM-call trace span with `TraceOutcome::Cancelled` before
        // dropping the losing stream future (aborting the underlying HTTP
        // stream). On cancellation the wrapper returns
        // `LlmError::Cancelled`; the partial assistant message is
        // synthesized from what has streamed so far, and the turn then
        // winds down through `tool_exec::run` (which appends the partial
        // to the history) and `decide` → `LoopDecision::ReturnCancelled`,
        // which persists it.
        match ctx
            .snapshot
            .llm_provider
            .complete_stream_scoped_with_cancel(
                runtime_view,
                &build_result.request,
                &on_chunk,
                ctx.state.cancel.cancelled(),
            )
            .await
        {
            Ok(response) => response,
            Err(LlmError::Cancelled) => {
                let partial_text = streamed_text
                    .lock()
                    .map(|text| text.clone())
                    .unwrap_or_default();
                let partial_reasoning = streamed_reasoning
                    .lock()
                    .map(|reasoning| reasoning.clone())
                    .unwrap_or_default();

                ctx.turn.ttft_ms = first_token_at.load(std::sync::atomic::Ordering::Relaxed);
                ctx.turn.total_time_ms = start.elapsed().as_millis() as u64;
                ctx.turn.tpot_ms = 0.0;

                if partial_text.is_empty() && partial_reasoning.is_empty() {
                    // Nothing streamed yet (Esc before the first token):
                    // mirror the entry-point cancel check above — no
                    // assistant message, `decide` still returns
                    // `ReturnCancelled` with the history as-is.
                    return Ok(());
                }

                // Flush the final partial to the sink when throttling (or
                // the delta fast path, which bypasses the throttle state)
                // left a tail behind — mirrors the post-stream flush below
                // so sink consumers see the same text that gets persisted.
                if let Some(ref sink) = event_sink {
                    let last_text_len = last_text_emit
                        .lock()
                        .map(|state| state.last_emit_len())
                        .unwrap_or(0);
                    if !partial_text.is_empty() && last_text_len != partial_text.len() {
                        let filtered_text = filter_secrets_in_text(&partial_text, &secrets);
                        sink.on_assistant_message(&agent_id, &filtered_text);
                    }
                    let last_reasoning_len = last_reasoning_emit
                        .lock()
                        .map(|state| state.last_emit_len())
                        .unwrap_or(0);
                    if !partial_reasoning.is_empty()
                        && last_reasoning_len != partial_reasoning.len()
                    {
                        let filtered_reasoning =
                            filter_secrets_in_text(&partial_reasoning, &secrets);
                        sink.on_assistant_reasoning(&agent_id, &filtered_reasoning);
                    }
                }

                tracing::info!(
                    text_len = partial_text.len(),
                    reasoning_len = partial_reasoning.len(),
                    "LLM stream cancelled mid-flight; persisting partial assistant message"
                );
                ctx.turn.assistant_message = Some(AssistantMessage {
                    text: (!partial_text.is_empty()).then_some(partial_text),
                    reasoning_content: (!partial_reasoning.is_empty()).then_some(partial_reasoning),
                    // The stream was cut mid-flight: tool calls (had any
                    // started arriving) are incomplete and discarded, and
                    // usage is unknown — report zeros rather than stale
                    // values from a previous turn.
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                    // The partial never ended a turn naturally; EndTurn is
                    // the closest variant (stop_reason is not persisted
                    // into the message history).
                    stop_reason: StopReason::EndTurn,
                });
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    };

    let total_time_ms = start.elapsed().as_millis() as u64;
    let ttft_ms = if std::env::var("XIAOO_NON_STREAMING").is_ok() {
        total_time_ms
    } else {
        first_token_at.load(std::sync::atomic::Ordering::Relaxed)
    };
    let completion_tokens = response.message.usage.completion_tokens;
    let tpot_ms = if ttft_ms > 0 && completion_tokens > 0 {
        (total_time_ms - ttft_ms) as f64 / completion_tokens as f64
    } else {
        0.0
    };

    ctx.turn.ttft_ms = ttft_ms;
    ctx.turn.total_time_ms = total_time_ms;
    ctx.turn.tpot_ms = tpot_ms;

    ctx.state.token_usage.prompt_tokens = response.message.usage.prompt_tokens;
    ctx.state.token_usage.completion_tokens = completion_tokens;
    ctx.state.token_usage.total_tokens = response.message.usage.total_tokens;
    ctx.state.token_usage.cached_tokens = response.message.usage.cached_tokens;

    // Flush the final filtered text/reasoning only when the last in-stream
    // emit did not already cover the full content (throttling may have
    // skipped the last few chunks, or no in-stream emit happened — e.g.
    // non-streaming path).
    if let Some(ref sink) = event_sink {
        let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref());
        if let Some(ref text) = response.message.text {
            let last_emit_len = last_text_emit
                .lock()
                .map(|state| state.last_emit_len())
                .unwrap_or(0);
            if last_emit_len != text.len() {
                let filtered_text = filter_secrets_in_text(text, &secrets);
                sink.on_assistant_message(agent_id, &filtered_text);
            }
        }
        if let Some(ref reasoning) = response.message.reasoning_content {
            let last_emit_len = last_reasoning_emit
                .lock()
                .map(|state| state.last_emit_len())
                .unwrap_or(0);
            if last_emit_len != reasoning.len() {
                let filtered_reasoning = filter_secrets_in_text(reasoning, &secrets);
                sink.on_assistant_reasoning(agent_id, &filtered_reasoning);
            }
        }
    }

    ctx.turn.assistant_message = Some(response.message);

    if ctx.snapshot.feature_flags.kvcache_enabled {
        let deleted_hashes = ctx
            .state
            .kv_cache_map
            .diff_deleted(&response.kv_cache_chunk_hashes);
        spawn_evict(deleted_hashes);

        let assistant_text = ctx
            .turn
            .assistant_message
            .as_ref()
            .and_then(|m| m.text.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("");
        ctx.state
            .kv_cache_map
            .replace(&response.kv_cache_chunk_hashes, assistant_text);

        if !response.kv_cache_chunk_hashes.is_empty()
            && ctx.snapshot.feature_flags.kvcache_debug_enabled
        {
            let cumulative_turn = ctx.state.turn_count + 1;
            let messages: Vec<serde_json::Value> = build_result
                .request
                .messages
                .iter()
                .map(|m| {
                    let blocks: Vec<serde_json::Value> = m
                        .blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => {
                                serde_json::json!({"type": "text", "text": text})
                            }
                            ContentBlock::ToolUse {
                                call_id,
                                tool_name,
                                input,
                            } => {
                                serde_json::json!({
                                    "type": "tool_use",
                                    "call_id": call_id,
                                    "tool_name": tool_name,
                                    "input": input,
                                })
                            }
                            ContentBlock::ToolResult {
                                call_id,
                                tool_name,
                                output,
                                is_error,
                            } => {
                                serde_json::json!({
                                    "type": "tool_result",
                                    "call_id": call_id,
                                    "tool_name": tool_name,
                                    "output": output,
                                    "is_error": is_error,
                                })
                            }
                            ContentBlock::Image { description } => {
                                serde_json::json!({"type": "image", "description": description})
                            }
                            ContentBlock::Document { description } => {
                                serde_json::json!({"type": "document", "description": description})
                            }
                        })
                        .collect();
                    let mut msg_json = serde_json::json!({
                        "role": m.role.as_str(),
                        "blocks": blocks,
                    });
                    if let Some(ref rc) = m.reasoning_content {
                        msg_json["reasoning_content"] = serde_json::json!(rc);
                    }
                    msg_json
                })
                .collect();
            let debug_entry = serde_json::json!({
                "session_id": ctx.state.session_id.clone(),
                "turn": cumulative_turn,
                "messages": messages,
                "chunk_hashes": response.kv_cache_chunk_hashes,
                "timing": {
                    "ttft_ms": ctx.turn.ttft_ms,
                    "total_time_ms": ctx.turn.total_time_ms,
                    "tpot_ms": ctx.turn.tpot_ms,
                },
            });
            let dir = std::path::Path::new("kvcache_debug");
            let _ = std::fs::create_dir_all(dir);
            let filename = format!(
                "kvcache_debug_{}_{}.json",
                ctx.state.session_id, cumulative_turn
            );
            let path = dir.join(&filename);
            if let Ok(json) = serde_json::to_string_pretty(&debug_entry) {
                let _ = std::fs::write(&path, json);
                tracing::info!(path = %path.display(), "kvcache debug file written");
            }
        }
    }

    Ok(())
}

const MAX_TRANSIENT_RETRIES: u32 = 4;
const TRANSIENT_BASE_DELAY_MS: u64 = 4_000;
const TRANSIENT_MAX_DELAY_MS: u64 = 60_000;

fn is_transient(error: &LlmError) -> bool {
    matches!(
        error,
        LlmError::RateLimited { .. }
            | LlmError::HttpError(_)
            | LlmError::Timeout
            | LlmError::StreamError { .. }
            | LlmError::IoError(_)
    )
}

fn transient_backoff(attempt: u32, retry_after_ms: u64) -> Duration {
    let millis = if retry_after_ms > 0 {
        retry_after_ms
    } else {
        TRANSIENT_BASE_DELAY_MS.saturating_mul(1u64 << attempt.min(63))
    };
    Duration::from_millis(millis.min(TRANSIENT_MAX_DELAY_MS))
}

async fn llm_call_with_recovery(ctx: &mut LoopContext<'_>) -> Result<(), AgentError> {
    let mut retry_attempts: u32 = 0;
    loop {
        match llm_call(ctx).await {
            Ok(()) => return Ok(()),
            Err(LlmError::ContextLengthExceeded { message }) => {
                tracing::warn!(
                    turn = ctx.turn.turn_number,
                    "LLM request exceeded provider context limit; forcing compression retry: {message}"
                );

                compress(ctx, CompressionTrigger::ContextLimitRetry).await?;
                build_messages(ctx).await?;
                return llm_call(ctx)
                    .await
                    .map_err(|error| AgentError::LlmProvider(error.to_string()));
            }
            Err(error) if retry_attempts < MAX_TRANSIENT_RETRIES && is_transient(&error) => {
                let retry_after_ms = match &error {
                    LlmError::RateLimited { retry_after_ms, .. } => *retry_after_ms,
                    _ => 0,
                };
                let backoff = transient_backoff(retry_attempts, retry_after_ms);
                retry_attempts += 1;
                tracing::warn!(
                    turn = ctx.turn.turn_number,
                    attempt = retry_attempts,
                    max_attempts = MAX_TRANSIENT_RETRIES,
                    backoff_ms = backoff.as_millis() as u64,
                    "transient LLM error; backing off before retrying agent turn: {error}"
                );
                tokio::select! {
                    _ = ctx.state.cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(backoff) => {}
                }
            }
            Err(error) => return Err(AgentError::LlmProvider(error.to_string())),
        }
    }
}

// Throttle parameters for in-stream sink emits. Without throttling, every
// chunk forces an O(history-length) clone + filter pass, making streaming
// O(N²) in the response length.
//   - emit when >= `MIN_DELTA_CHARS` new chars accumulated since last emit, OR
//   - emit when `MAX_LATENCY` elapsed since last emit (slow streams still progress).
const STREAM_EMIT_MIN_DELTA_CHARS: usize = 32;
const STREAM_EMIT_MAX_LATENCY: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Default)]
struct StreamEmitState {
    last_emit_len: usize,
    last_emit_at: Option<std::time::Instant>,
}

impl StreamEmitState {
    fn should_emit(&mut self, current_len: usize) -> bool {
        let now = std::time::Instant::now();
        let delta_ok =
            current_len.saturating_sub(self.last_emit_len) >= STREAM_EMIT_MIN_DELTA_CHARS;
        let latency_ok = self
            .last_emit_at
            .is_some_and(|t| now.duration_since(t) >= STREAM_EMIT_MAX_LATENCY);
        let should = delta_ok || latency_ok;
        if should {
            self.last_emit_len = current_len;
            self.last_emit_at = Some(now);
        }
        should
    }

    /// Length of streamed content at the last in-stream emit (0 if none).
    /// Used by the post-stream flush to skip when the last emit already
    /// covered the full content.
    fn last_emit_len(&self) -> usize {
        self.last_emit_len
    }
}

fn stream_assistant_chunk(
    sink: Option<&dyn LoopEventSink>,
    agent_id: &AgentId,
    streamed_text: &Mutex<String>,
    streamed_reasoning: &Mutex<String>,
    last_text_emit: &Mutex<StreamEmitState>,
    last_reasoning_emit: &Mutex<StreamEmitState>,
    chunk: StreamChunk,
    secrets: &[String],
) {
    // Always accumulate into the streamed_* buffers, even without a sink:
    // the cancel branch of `llm_call` (Esc racing the in-flight stream)
    // synthesizes the partial assistant message from them. The emits below
    // are individually guarded on `sink`, so a sinkless call only pays the
    // accumulation; the post-stream flush still reads
    // `response.message.text` / `reasoning_content` directly.

    #[cfg(debug_assertions)]
    let _start = std::time::Instant::now();
    #[cfg(debug_assertions)]
    let mut reasoning_len = 0usize;
    #[cfg(debug_assertions)]
    let mut text_len = 0usize;

    if let Some(delta_reasoning) = chunk.delta_reasoning {
        #[cfg(debug_assertions)]
        {
            reasoning_len = delta_reasoning.len();
        }
        let current_len = {
            let mut full_reasoning = streamed_reasoning
                .lock()
                .expect("assistant stream reasoning mutex should not be poisoned");
            full_reasoning.push_str(&delta_reasoning);
            // Release the streamed_reasoning lock before acquiring the emit
            // state lock to avoid lock-ordering issues.
            full_reasoning.len()
        };
        if let Some(sink) = sink {
            // Delta fast path: O(1) per chunk, no history clone. Only safe
            // when no secrets need redaction — a secret split across a chunk
            // boundary would otherwise leak its already-emitted prefix, since
            // the append-only delta API cannot express a "replace prefix with
            // <SECRET>" correction. Delta is cheap (independent of history
            // length) so it bypasses the throttle below.
            if sink.supports_message_delta() && secrets.is_empty() {
                sink.on_assistant_reasoning_delta(agent_id, &delta_reasoning);
            } else {
                let should_emit = last_reasoning_emit
                    .lock()
                    .expect("assistant stream reasoning emit state mutex should not be poisoned")
                    .should_emit(current_len);
                if should_emit {
                    let snapshot = streamed_reasoning
                        .lock()
                        .expect("assistant stream reasoning mutex should not be poisoned")
                        .clone();
                    let filtered_reasoning = filter_secrets_in_text(&snapshot, secrets);
                    sink.on_assistant_reasoning(agent_id, &filtered_reasoning);
                }
            }
        }
    }

    if let Some(delta_text) = chunk.delta_text {
        #[cfg(debug_assertions)]
        {
            text_len = delta_text.len();
        }
        let current_len = {
            let mut full_text = streamed_text
                .lock()
                .expect("assistant stream text mutex should not be poisoned");
            full_text.push_str(&delta_text);
            full_text.len()
        };
        if let Some(sink) = sink {
            if sink.supports_message_delta() && secrets.is_empty() {
                sink.on_assistant_message_delta(agent_id, &delta_text);
            } else {
                let should_emit = last_text_emit
                    .lock()
                    .expect("assistant stream text emit state mutex should not be poisoned")
                    .should_emit(current_len);
                if should_emit {
                    let snapshot = streamed_text
                        .lock()
                        .expect("assistant stream text mutex should not be poisoned")
                        .clone();
                    let filtered_text = filter_secrets_in_text(&snapshot, secrets);
                    sink.on_assistant_message(agent_id, &filtered_text);
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    tracing::debug!(
        target: "perf",
        delta_text_len = text_len,
        delta_reasoning_len = reasoning_len,
        supports_delta = sink.is_some_and(|s| s.supports_message_delta()),
        accumulated_text_len = streamed_text.lock().map(|t| t.len()).unwrap_or(0),
        elapsed_us = _start.elapsed().as_micros(),
        "stream_assistant_chunk"
    );
}

/// Extract secret values from message history for filtering in assistant messages
fn extract_secrets_from_messages(messages: &[ChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .flat_map(|m| m.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_name, output, ..
            } => {
                if tool_name == "ask_user_question" {
                    Some(output)
                } else {
                    None
                }
            }
            _ => None,
        })
        .filter_map(|output| serde_json::from_str::<serde_json::Value>(output).ok())
        .filter_map(|json| json.get("answers").and_then(|a| a.as_array()).cloned())
        .flatten()
        .filter_map(|answer| {
            let is_text = answer.get("kind").and_then(|k| k.as_str()) == Some("text");
            let value = answer.get("value").and_then(|v| v.as_str());
            // Only extract as secret if has display_value field (is_secret=true was used)
            let has_display_value = answer
                .get("display_value")
                .map(|v| !v.is_null())
                .unwrap_or(false);

            if is_text && has_display_value && value.map(|v| !v.is_empty()).unwrap_or(false) {
                value.map(|v| v.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Filter secrets (passwords) in text by replacing them with <SECRET>
fn filter_secrets_in_text(text: &str, secrets: &[String]) -> String {
    let mut filtered = text.to_string();
    for secret in secrets {
        if filtered.contains(secret) {
            filtered = filtered.replace(secret, "<SECRET>");
        }
    }
    filtered
}

fn decide(ctx: &mut LoopContext<'_>) {
    if ctx.state.cancel.is_cancelled() {
        ctx.turn.decision = Some(LoopDecision::ReturnCancelled);
        return;
    }

    if ctx.turn.force_return_complete {
        ctx.turn.decision = Some(LoopDecision::ReturnComplete);
        return;
    }

    // NOTE: no cumulative token budget check here.
    // Context window pressure is handled by the compression pipeline
    // (compress/microcompact) at the start of each turn.

    // If we've reached the max_turns limit, end the loop now regardless of
    // whether the assistant produced tool calls. On the final turn, tools
    // are withheld during prompt building (see `build_messages`) so the
    // model commits a text-only final answer rather than a cut-off tool
    // call; we still must surface `MaxTurnsReached` here so downstream
    // consumers (e.g. the `*.Session.lifecycle.state` plugin hook) see the
    // soft-termination outcome instead of a normal `Complete`. Without
    // this guard the loop would fall through to `ReturnComplete` whenever
    // the model complied with the no-tools final turn, making the
    // `max_turns_reached` outcome unreachable for any agent whose limit
    // is hit on a turn that yields text.
    if ctx.snapshot.max_turns > 0 && ctx.turn.turn_number >= ctx.snapshot.max_turns {
        ctx.turn.decision = Some(LoopDecision::ReturnMaxTurns);
        return;
    }

    if let Some(ref msg) = ctx.turn.assistant_message {
        let can_execute_tool_calls = ctx.snapshot.feature_flags.tool_execution
            && !ctx.input.visible_tools.is_empty()
            && ctx.input.runtime_view.is_some();

        if msg.has_tool_calls() && can_execute_tool_calls {
            ctx.turn.decision = Some(LoopDecision::Continue);
            return;
        }
    }

    // Don't accept a stop while the model still has open plan items. The first
    // such stop triggers one reminder (bounded by `plan_nudged`, so never an
    // infinite loop — if the model stops again it completes). Only fires when the
    // model actually used `todo_write` and left items open.
    if !ctx.state.plan_nudged && ctx.turn.turn_number < ctx.snapshot.max_turns {
        let open = ctx
            .input
            .runtime_view
            .as_ref()
            .map(|runtime_view| tool::open_todo_lines(runtime_view.as_ref()))
            .unwrap_or_default();
        if !open.is_empty() {
            ctx.state.plan_nudged = true;
            let reminder = format!(
                "You are about to stop, but your plan still has {} open item(s):\n{}\n\
                 Finish them now, or call todo_write to mark them completed/cancelled if they no longer apply — then stop.",
                open.len(),
                open.join("\n")
            );
            let mut msg = ChatMessage::user(reminder);
            ctx.estimator.update_message_cache(&mut msg);
            ctx.state.messages.write().push(msg);
            ctx.turn.decision = Some(LoopDecision::Continue);
            return;
        }
    }

    ctx.turn.decision = Some(LoopDecision::ReturnComplete);
}

fn append_assistant_to_history(ctx: &mut LoopContext<'_>) {
    let msg = match ctx.turn.assistant_message {
        Some(ref msg) => msg,
        None => return,
    };

    let mut blocks = Vec::new();

    if let Some(ref text) = msg.text {
        blocks.push(ContentBlock::Text { text: text.clone() });
    }

    for tc in &msg.tool_calls {
        blocks.push(ContentBlock::ToolUse {
            call_id: tc.call_id.clone(),
            tool_name: tc.tool_name.clone(),
            input: tc.input.clone(),
        });
    }

    let mut msg = ChatMessage {
        role: MessageRole::Assistant,
        blocks,
        message_id: None,
        timestamp_ms: now_ms(),
        api_usage_tokens: Some(msg.usage.total_tokens),
        reasoning_content: msg.reasoning_content.clone(),
        estimated_tokens: None,
    };
    ctx.estimator.update_message_cache(&mut msg);
    ctx.state.messages.write().push(msg);
}

fn emit_loop_end(ctx: &LoopContext<'_>, stop_reason: &str) {
    if let Some(ref sink) = ctx.input.event_sink {
        let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref());
        sink.on_loop_end(
            agent_id,
            &agent_types::events::LoopEndSummary {
                turn_count: ctx.state.turn_count,
                total_tokens: ctx.state.token_usage.total_tokens,
                stop_reason: stop_reason.into(),
            },
        );
    }
}

/// Detect `/skill-name [args]` prefix in user message and expand to skill prompt.
///
/// Returns `Some(expanded_message)` if a valid skill invocation is detected,
/// `None` otherwise (message is passed through unchanged).
fn try_expand_skill_prefix(
    user_message: &str,
    skill_registry: &dyn agent_contracts::SkillRegistry,
) -> Option<String> {
    let trimmed = user_message.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    // Extract skill name (first token after '/') and remaining args.
    let without_slash = &trimmed[1..];
    let (skill_name, args) = match without_slash.find(|c: char| c.is_whitespace()) {
        Some(pos) => (&without_slash[..pos], without_slash[pos..].trim()),
        None => (without_slash, ""),
    };

    if skill_name.is_empty() {
        return None;
    }

    let spec = skill_registry.get_skill(skill_name)?;

    if !spec.user_invocable() {
        return None;
    }

    let mut expanded = String::new();

    // Provide the skill directory so the LLM knows where to run commands.
    if let Some(location) = spec.location() {
        expanded.push_str(&format!("[Skill directory: {}]\n\n", location.display()));
    }

    expanded.push_str(spec.full_prompt());

    if !args.is_empty() {
        expanded.push_str("\n\nUser request: ");
        expanded.push_str(args);
    }

    Some(expanded)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn build_outcome_max_turns(ctx: &LoopContext<'_>) -> AgentOutcome {
    AgentOutcome::MaxTurnsReached {
        partial_reply: ctx
            .turn
            .assistant_message
            .as_ref()
            .and_then(|m| m.text.clone()),
        messages: ctx.state.messages.read().clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
        estimated_input_tokens: current_turn_estimated_input_tokens(ctx),
    }
}

fn build_outcome_budget(ctx: &LoopContext<'_>) -> AgentOutcome {
    AgentOutcome::BudgetExhausted {
        partial_reply: ctx
            .turn
            .assistant_message
            .as_ref()
            .and_then(|m| m.text.clone()),
        messages: ctx.state.messages.read().clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
        estimated_input_tokens: current_turn_estimated_input_tokens(ctx),
    }
}

fn build_outcome_cancelled(ctx: &LoopContext<'_>) -> AgentOutcome {
    AgentOutcome::Cancelled {
        partial_reply: ctx
            .turn
            .assistant_message
            .as_ref()
            .and_then(|m| m.text.clone()),
        messages: ctx.state.messages.read().clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
        estimated_input_tokens: current_turn_estimated_input_tokens(ctx),
    }
}

fn current_turn_estimated_input_tokens(ctx: &LoopContext<'_>) -> usize {
    ctx.turn
        .build_messages_output
        .as_ref()
        .map(|result| result.estimated_input_tokens)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../../../tests/unit/core/agent_loop_test.rs"]
mod basics_test;
