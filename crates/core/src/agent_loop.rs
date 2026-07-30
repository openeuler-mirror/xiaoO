use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use agent_contracts::context::prompt::input::PromptBuildInput;
use agent_contracts::events::LoopEventSink;
use agent_contracts::tool::ToolCallBuilder;
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_contracts::{Hooker, RuntimeView};
use agent_llm::{AssistantMessageExt, ChatMessageExt, MessageRoleExt};
use agent_types::chat::{ChatSystemTransformInput, ChatSystemTransformResult, ModelRef};
use agent_types::compression::CompressedView;
use agent_types::context::prompt::result::PromptBuildResult;
use agent_types::events::ToolResultEvent;
use agent_types::hook::{
    HookInvokeInput, HookInvokeMetadata, HookInvokeOutput, HookInvokePrimary, HookPointId,
};
use agent_types::outcome::{AgentError, AgentOutcome};
use agent_types::tool::{EffectProfile, RawToolCall, RawToolOutcome, ToolExecutionResult};
use agent_types::{
    AgentId, AssistantMessage, ChatMessage, ContentBlock, LlmError, MessageRole, StreamChunk,
    ToolUseBlock,
};
use serde_json::{json, Value};
use tool::{tool_filter_from_specs, ToolCallBuilderImpl};

use crate::input::{AgentLoopInput, LoopStopRule};
use crate::loop_state::LoopState;
use crate::runtime::AgentRuntime;
use crate::snapshot::RuntimeSnapshot;
use crate::suspend::{LoopRunResult, SuspendedToolCall};
use crate::token_estimator::TokenEstimator;

use crate::spawn_evict;

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
                    let deny_msg = ChatMessage::text(MessageRole::Assistant, &deny_text, now_ms());
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
        let final_message = apply_chat_message_hook(
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
        state.messages.write().push(final_message);
    }

    let mut ctx = LoopContext {
        snapshot,
        state,
        input,
        turn: TurnState::new(1),
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
        let suspended_calls = match tool_exec(&mut ctx).await {
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

    Ok(LoopRunResult::Complete(AgentOutcome::Complete {
        reply,
        messages: ctx.state.messages.read().clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
        estimated_input_tokens: current_turn_estimated_input_tokens(&ctx),
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
        let final_message = apply_chat_message_hook(
            ctx.input.runtime_view.as_ref(),
            &ctx.state.session_id,
            ctx.input.agent_id.as_ref(),
            candidate,
        )
        .await;
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
    let messages = ctx.state.messages.read().clone();
    let estimated_input = estimator.estimate_input_tokens(
        &ctx.snapshot.system_prompt,
        ctx.snapshot.tool_registry.list_specs().len(),
        &messages,
    );

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

    let new_messages = ctx.state.messages.read().clone();
    let new_estimated = estimator.estimate_input_tokens(
        &ctx.snapshot.system_prompt,
        ctx.snapshot.tool_registry.list_specs().len(),
        &new_messages,
    );

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

    // Clone messages before potential .await to avoid holding RwLockReadGuard across await point
    let messages = ctx.state.messages.read().clone();
    let analysis = ctx
        .snapshot
        .compression_pipeline
        .analyze(&messages, &*ctx.snapshot.token_budget_policy);

    tracing::debug!(
        estimated = analysis.estimated_tokens,
        available = analysis.available_tokens,
        ratio = format!("{:.1}%", analysis.usage_ratio * 100.0),
        severity = ?analysis.severity,
        msg_count = messages.len(),
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
        Ok(view) => {
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

            *ctx.state.messages.write() = view.messages.clone();
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

/// Per-turn dynamic context re-injected into the system prompt: the remaining
/// horizon and the live `todo_write` plan. Rendered into the volatile tail of
/// the system message (see `prompt::compose`), after the cache-stable prefix.
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

    let mut projected_messages = ctx.state.messages.read().clone();
    prune_stale_tool_output(&mut projected_messages);

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
    let filtered_text_len = Mutex::new(0usize);
    let filtered_reasoning_len = Mutex::new(0usize);

    // Extract secrets from message history to filter in assistant messages
    let messages = ctx.state.messages.read().clone();
    let secrets = extract_secrets_from_messages(&messages);

    let runtime_view = ctx.input.runtime_view.as_deref();
    let response = if std::env::var("XIAOO_NON_STREAMING").is_ok() {
        ctx.snapshot
            .llm_provider
            .complete_scoped(runtime_view, &build_result.request)
            .await?
    } else {
        let first_token_at = std::sync::Arc::clone(&first_token_at);
        ctx.snapshot
            .llm_provider
            .complete_stream_scoped(runtime_view, &build_result.request, &|chunk| {
                if first_token_at.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                    first_token_at.store(
                        start.elapsed().as_millis() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref()).clone();
                stream_assistant_chunk(
                    event_sink.as_deref(),
                    &agent_id,
                    &streamed_text,
                    &streamed_reasoning,
                    &filtered_text_len,
                    &filtered_reasoning_len,
                    chunk,
                    &secrets,
                );
            })
            .await?
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

    let streamed_text = streamed_text
        .into_inner()
        .expect("assistant stream text mutex should not be poisoned");
    if let Some(ref sink) = event_sink {
        if let Some(ref text) = response.message.text {
            if streamed_text != *text {
                let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref());
                let filtered_text = filter_secrets_in_text(text, &secrets);
                sink.on_assistant_message(agent_id, &filtered_text);
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

fn stream_assistant_chunk(
    sink: Option<&dyn LoopEventSink>,
    agent_id: &AgentId,
    streamed_text: &Mutex<String>,
    streamed_reasoning: &Mutex<String>,
    filtered_text_len: &Mutex<usize>,
    filtered_reasoning_len: &Mutex<usize>,
    chunk: StreamChunk,
    secrets: &[String],
) {
    #[cfg(debug_assertions)]
    let _start = std::time::Instant::now();
    #[cfg(debug_assertions)]
    let mut reasoning_len = 0usize;
    #[cfg(debug_assertions)]
    let mut text_len = 0usize;

    if let Some(delta_reasoning) = chunk.delta_reasoning {
        #[cfg(debug_assertions)]
        { reasoning_len = delta_reasoning.len(); }
        let mut full_reasoning = streamed_reasoning
            .lock()
            .expect("assistant stream reasoning mutex should not be poisoned");
        full_reasoning.push_str(&delta_reasoning);

        if let Some(sink) = sink {
            if sink.supports_message_delta() {
                if secrets.is_empty() {
                    sink.on_assistant_reasoning_delta(agent_id, &delta_reasoning);
                } else {
                    let filtered_full = filter_secrets_in_text(&full_reasoning, secrets);
                    let mut prev_len = filtered_reasoning_len
                        .lock()
                        .expect("filtered reasoning len mutex should not be poisoned");
                    if *prev_len < filtered_full.len() {
                        let delta = &filtered_full[*prev_len..];
                        if !delta.is_empty() {
                            sink.on_assistant_reasoning_delta(agent_id, delta);
                        }
                    } else if *prev_len > filtered_full.len() {
                        sink.on_assistant_reasoning(agent_id, &filtered_full);
                    }
                    *prev_len = filtered_full.len();
                }
            } else {
                let snapshot = full_reasoning.clone();
                let filtered = filter_secrets_in_text(&snapshot, secrets);
                sink.on_assistant_reasoning(agent_id, &filtered);
            }
        }
    }

    if let Some(delta_text) = chunk.delta_text {
        #[cfg(debug_assertions)]
        { text_len = delta_text.len(); }
        let mut full_text = streamed_text
            .lock()
            .expect("assistant stream text mutex should not be poisoned");
        full_text.push_str(&delta_text);

        if let Some(sink) = sink {
            if sink.supports_message_delta() {
                if secrets.is_empty() {
                    sink.on_assistant_message_delta(agent_id, &delta_text);
                } else {
                    let filtered_full = filter_secrets_in_text(&full_text, secrets);
                    let mut prev_len = filtered_text_len
                        .lock()
                        .expect("filtered text len mutex should not be poisoned");
                    if *prev_len < filtered_full.len() {
                        let delta = &filtered_full[*prev_len..];
                        if !delta.is_empty() {
                            sink.on_assistant_message_delta(agent_id, delta);
                        }
                    } else if *prev_len > filtered_full.len() {
                        sink.on_assistant_message(agent_id, &filtered_full);
                    }
                    *prev_len = filtered_full.len();
                }
            } else {
                let snapshot = full_text.clone();
                let filtered = filter_secrets_in_text(&snapshot, secrets);
                sink.on_assistant_message(agent_id, &filtered);
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

async fn tool_exec(ctx: &mut LoopContext<'_>) -> Result<Vec<SuspendedToolCall>, AgentError> {
    let has_tool_calls = ctx
        .turn
        .assistant_message
        .as_ref()
        .map_or(false, |m| m.has_tool_calls());

    if ctx.turn.assistant_message.is_none() {
        return Ok(Vec::new());
    }

    if !has_tool_calls || !ctx.snapshot.feature_flags.tool_execution {
        append_assistant_to_history(ctx);
        return Ok(Vec::new());
    }

    if ctx.input.runtime_view.is_none() {
        append_assistant_to_history(ctx);
        return Ok(Vec::new());
    }

    // Repair empty call_ids before the validity partition below.
    if let Some(msg) = ctx.turn.assistant_message.as_mut() {
        synthesize_missing_call_ids(msg, ctx.state.turn_count);
        repair_tool_names(msg, &ctx.input.visible_tools);
    }

    let tool_calls: Vec<ToolUseBlock> = ctx
        .turn
        .assistant_message
        .as_ref()
        .unwrap()
        .tool_calls
        .clone();

    if ctx.input.agent_id.is_none() {
        return Ok(Vec::new());
    }

    // Partition tool calls into valid (non-empty call_id + tool_name) and invalid.
    let (valid_calls, invalid_calls): (Vec<_>, Vec<_>) =
        tool_calls.into_iter().partition(is_valid_tool_call);

    // Handle the case where ALL tool calls are invalid and there is exactly one:
    // retry the LLM request once, but only if we can safely synthesize a tool_result.
    let (valid_calls, invalid_calls) = if valid_calls.is_empty() && invalid_calls.len() == 1 {
        let invalid_call = &invalid_calls[0];
        if can_retry_invalid_tool_call(invalid_call) {
            tracing::warn!(
                call_id = %invalid_call.call_id,
                tool_name = %invalid_call.tool_name,
                "LLM returned a single invalid tool call; retrying LLM request"
            );

            // Inject a temporary error tool_result so the model can recover with a
            // corrected tool call, but do not keep that synthetic message in history.
            ctx.state
                .messages
                .write()
                .push(build_invalid_tool_call_result(invalid_call));
            let retry_result = async {
                build_messages(ctx).await.map_err(|e| {
                    AgentError::PromptBuild(format!("retry after invalid tool call: {e}"))
                })?;
                llm_call_with_recovery(ctx).await
            }
            .await;
            ctx.state.messages.write().pop();
            retry_result?;

            let retry_tool_calls: Vec<ToolUseBlock> = ctx
                .turn
                .assistant_message
                .as_ref()
                .map(|m| m.tool_calls.clone())
                .unwrap_or_default();

            let (retry_valid, retry_invalid): (Vec<_>, Vec<_>) =
                retry_tool_calls.into_iter().partition(is_valid_tool_call);

            if retry_valid.is_empty() && !retry_invalid.is_empty() {
                tracing::error!(
                    count = retry_invalid.len(),
                    "LLM returned invalid tool call(s) after retry; degrading to assistant text only"
                );
            }

            (retry_valid, retry_invalid)
        } else {
            tracing::warn!(
                call_id = %invalid_call.call_id,
                tool_name = %invalid_call.tool_name,
                "LLM returned a single invalid tool call without a retry-safe call_id; degrading to assistant text only"
            );
            (valid_calls, invalid_calls)
        }
    } else {
        (valid_calls, invalid_calls)
    };

    let mut valid_calls = valid_calls;
    valid_calls.sort_by_key(|tc| tc.tool_name == "join_subagent");

    if let Some(msg) = ctx.turn.assistant_message.as_mut() {
        msg.tool_calls = valid_calls.clone();
    }
    append_assistant_to_history(ctx);

    // Emit error events for invalid calls, but do not write them into history unless
    // we can safely pair them to a real tool_use call.
    for inv in &invalid_calls {
        tracing::warn!(
            call_id = %inv.call_id,
            tool_name = %inv.tool_name,
            "Discarding invalid tool call from LLM response"
        );

        if let Some(ref sink) = ctx.input.event_sink {
            let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref());

            let messages = ctx.state.messages.read().clone();
            let secrets = extract_secrets_from_messages(&messages);
            let args_preview =
                serde_json::to_string_pretty(&inv.input).unwrap_or_else(|_| inv.input.to_string());
            let filtered_args_preview = if inv.tool_name == "bash" {
                filter_bash_args_preview(&args_preview, &secrets)
            } else {
                args_preview
            };

            sink.on_tool_result(
                agent_id,
                &ToolResultEvent {
                    call_id: inv.call_id.clone(),
                    tool_name: inv.tool_name.clone(),
                    output_preview: invalid_tool_call_message(inv),
                    is_error: true,
                    args_preview: filtered_args_preview,
                },
            );
        }
    }

    // Pass 1 — build every call (borrows ctx for the per-call tool filter).
    let mut built = Vec::with_capacity(valid_calls.len());
    for tc in &valid_calls {
        let raw_tool_call = RawToolCall {
            call_id: tc.call_id.clone(),
            tool_name: tc.tool_name.clone(),
            input: tc.input.clone(),
        };
        let fallback_final_call = agent_types::tool::FinalToolCall {
            call_id: raw_tool_call.call_id.clone(),
            tool_name: raw_tool_call.tool_name.clone(),
            input: raw_tool_call.input.clone(),
        };

        let per_call_filter = tool_filter_from_specs(
            &ctx.input.visible_tools,
            ctx.snapshot.tool_registry.as_ref(),
        );

        match ToolCallBuilderImpl::new()
            .with_raw_llm_tool_call(raw_tool_call)
            .with_tool_filter(per_call_filter)
            .build()
        {
            Ok(tool_call) => built.push(Ok(tool_call)),
            Err(error) => built.push(Err(build_framework_failed_tool_result(
                fallback_final_call,
                format!("tool call build failed: {error}"),
            ))),
        }
    }

    let serialize_batch = {
        let profiles: std::collections::HashMap<&str, &EffectProfile> = ctx
            .input
            .visible_tools
            .iter()
            .map(|tool| (tool.name().0.as_str(), tool.effect_profile()))
            .collect();
        !built.iter().filter_map(|b| b.as_ref().ok()).all(|call| {
            profiles
                .get(call.final_call().tool_name.as_str())
                .is_some_and(|profile| is_parallel_safe(profile))
        })
    };

    // Pass 2 — execute the successfully built calls. Clone the Arc runtime handle
    // so the futures borrow it, not `ctx` (post-processing needs `&mut ctx`).
    // Both paths preserve input order, so Pass 3/4 are unaffected.
    let runtime_view = ctx.input.runtime_view.clone().unwrap();
    let exec_outcomes: Vec<_> = if serialize_batch {
        let mut outcomes = Vec::new();
        for call in built.iter().filter_map(|b| b.as_ref().ok()) {
            outcomes.push(call.execute(&*runtime_view).await);
        }
        outcomes
    } else {
        futures_util::future::join_all(
            built
                .iter()
                .filter_map(|b| b.as_ref().ok().map(|call| call.execute(&*runtime_view))),
        )
        .await
    };

    // Pass 3 — stitch outcomes back into call order, pairing each executed call
    // with its result (build failures already carry their own result).
    let mut exec_outcomes = exec_outcomes.into_iter();
    let mut results: Vec<ToolExecutionResult> = Vec::with_capacity(built.len());
    for entry in built {
        match entry {
            Err(failed_result) => results.push(failed_result),
            Ok(tool_call) => {
                let result = match exec_outcomes.next().expect("one outcome per executed call") {
                    Ok(result) => result,
                    Err(error) => build_framework_failed_tool_result(
                        tool_call.final_call().clone(),
                        error.to_string(),
                    ),
                };
                results.push(result);
            }
        }
    }

    // Pass 4 — record results in call order. Suspending calls (`join_subagent`)
    // are sorted last (above), so by the time the first one is seen every
    // side-effecting sibling already has its tool_result recorded.
    let mut streak_note: Option<String> = None;
    let mut suspended_calls: Vec<SuspendedToolCall> = Vec::new();
    let mut stop_after_batch = false;
    for result in results {
        ctx.state.tool_executed = true;
        if should_stop_after_tool_result(ctx, &result) {
            stop_after_batch = true;
        }
        emit_tool_result_event(ctx, &result);

        if let Some(suspended_call) = SuspendedToolCall::from_tool_result(&result) {
            // Defer: no tool_result message now (the resumer appends it once the
            // child finishes). Recording the raw result keeps tool_results complete.
            ctx.turn.tool_results.push(result);
            suspended_calls.push(suspended_call);
            continue;
        }

        let tool_result_message = build_tool_result_message(&result);
        ctx.state.messages.write().push(tool_result_message);
        // Track repeated identical failing calls; any note is pushed after all
        // tool results so the assistant/tool-result protocol stays intact.
        if let Some(note) = update_tool_failure_streak(ctx, &result) {
            streak_note = Some(note);
        }
        ctx.turn.tool_results.push(result);
    }

    if stop_after_batch && suspended_calls.is_empty() {
        ctx.turn.force_return_complete = true;
    }

    // A pending suspend must not be followed by an injected user message: the
    // resumer still has to slot tool_result(s) right after the assistant turn, so
    // hold the streak nudge until everything is resolved (drop it this turn).
    if suspended_calls.is_empty() {
        if let Some(note) = streak_note {
            ctx.state.messages.write().push(ChatMessage::user(note));
        }
    }

    Ok(suspended_calls)
}

const REPEATED_FAILURE_THRESHOLD: u32 = 3;
const REPEATED_SUCCESS_THRESHOLD: u32 = 3;

fn update_tool_failure_streak(
    ctx: &mut LoopContext<'_>,
    result: &ToolExecutionResult,
) -> Option<String> {
    let sig = tool_call_signature(result);
    if is_failure_result(result) {
        ctx.state.last_success_sig = None;
        ctx.state.repeated_success_count = 0;
        if ctx.state.last_failure_sig == Some(sig) {
            ctx.state.repeated_failure_count += 1;
        } else {
            ctx.state.last_failure_sig = Some(sig);
            ctx.state.repeated_failure_count = 1;
        }
        if ctx.state.repeated_failure_count >= REPEATED_FAILURE_THRESHOLD {
            let count = ctx.state.repeated_failure_count;
            let tool = result.tool_name().to_string();
            ctx.state.repeated_failure_count = 0;
            ctx.state.last_failure_sig = None;
            return Some(format!(
                "The `{tool}` call has now failed {count} times in a row with identical arguments. \
                 Stop retrying it unchanged — change approach: fix the arguments, read the relevant \
                 file or state to understand why it fails, or use a different tool to reach the goal."
            ));
        }
        return None;
    }
    ctx.state.last_failure_sig = None;
    ctx.state.repeated_failure_count = 0;
    if ctx.state.last_success_sig == Some(sig) {
        ctx.state.repeated_success_count += 1;
    } else {
        ctx.state.last_success_sig = Some(sig);
        ctx.state.repeated_success_count = 1;
    }
    if ctx.state.repeated_success_count >= REPEATED_SUCCESS_THRESHOLD {
        let count = ctx.state.repeated_success_count;
        let tool = result.tool_name().to_string();
        ctx.state.repeated_success_count = 0;
        ctx.state.last_success_sig = None;
        return Some(format!(
            "The `{tool}` call has now run {count} times in a row with identical arguments and the \
             same result — that output is already in your context above. Stop repeating it: use \
             what you have, or take a different action toward the goal."
        ));
    }
    None
}

fn is_parallel_safe(profile: &EffectProfile) -> bool {
    !profile.writes_filesystem
        && !profile.side_effects
        && (profile.reads_filesystem || profile.network_access)
}

fn is_failure_result(result: &ToolExecutionResult) -> bool {
    matches!(
        result,
        ToolExecutionResult::Completed {
            raw_outcome: RawToolOutcome::Error { .. },
            ..
        } | ToolExecutionResult::Failed { .. }
            | ToolExecutionResult::Denied { .. }
    )
}

fn tool_call_signature(result: &ToolExecutionResult) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    result.tool_name().hash(&mut hasher);
    serde_json::to_string(&result.final_call().input)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// Fill empty `call_id`s with a stable, turn-scoped id (`call_<turn>_<idx>`) so a
/// provider that omits `tool_call.id` does not get the call rejected as invalid; a
/// non-empty id is left untouched.
fn synthesize_missing_call_ids(msg: &mut AssistantMessage, turn: u32) {
    for (idx, tc) in msg.tool_calls.iter_mut().enumerate() {
        if tc.call_id.trim().is_empty() {
            tc.call_id = format!("call_{turn}_{idx}");
        }
    }
}

fn repair_tool_names(
    msg: &mut AssistantMessage,
    visible: &[std::sync::Arc<dyn agent_contracts::tool::ToolSpecView>],
) {
    if visible.is_empty() {
        return;
    }
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let mut canonical = std::collections::HashSet::new();
    let mut normalized = std::collections::HashMap::new();
    for tool in visible {
        let name = tool.name().0.clone();
        normalized
            .entry(normalize(&name))
            .or_insert_with(|| name.clone());
        canonical.insert(name);
    }
    for tc in msg.tool_calls.iter_mut() {
        if canonical.contains(&tc.tool_name) {
            continue;
        }
        if let Some(fixed) = normalized.get(&normalize(&tc.tool_name)) {
            if *fixed != tc.tool_name {
                tracing::debug!(from = %tc.tool_name, to = %fixed, "repaired tool name");
                tc.tool_name = fixed.clone();
            }
        }
    }
}

fn is_valid_tool_call(tc: &ToolUseBlock) -> bool {
    is_valid_tool_call_id(&tc.call_id) && is_valid_tool_name(&tc.tool_name)
}

fn is_valid_tool_call_id(call_id: &str) -> bool {
    !call_id.trim().is_empty()
}

fn is_valid_tool_name(name: &str) -> bool {
    !name.trim().is_empty()
}

fn can_retry_invalid_tool_call(tc: &ToolUseBlock) -> bool {
    is_valid_tool_call_id(&tc.call_id)
}

fn invalid_tool_call_message(tc: &ToolUseBlock) -> String {
    match (
        is_valid_tool_call_id(&tc.call_id),
        is_valid_tool_name(&tc.tool_name),
    ) {
        (false, false) => "invalid tool call: missing call_id and tool_name".to_string(),
        (false, true) => "invalid tool call: missing call_id".to_string(),
        (true, false) => "invalid tool call: missing tool_name".to_string(),
        (true, true) => "invalid tool call".to_string(),
    }
}

/// Build an error tool_result message for a tool call whose metadata was invalid.
/// This is only safe when the call_id is present, so the model can pair the result.
fn build_invalid_tool_call_result(tc: &ToolUseBlock) -> ChatMessage {
    ChatMessage::tool_result(
        tc.call_id.clone(),
        tc.tool_name.clone(),
        format!("Error: {}.", invalid_tool_call_message(tc)),
        true,
        now_ms(),
    )
}

fn build_framework_failed_tool_result(
    final_call: agent_types::tool::FinalToolCall,
    message: String,
) -> ToolExecutionResult {
    ToolExecutionResult::Failed {
        final_call,
        pre_hook_results: Vec::new(),
        error_hook_results: Vec::new(),
        execution_error: agent_types::tool::ToolExecutionError::ExecutionFailed { message },
    }
}

fn emit_tool_result_event(ctx: &LoopContext<'_>, result: &ToolExecutionResult) {
    let Some(ref sink) = ctx.input.event_sink else {
        return;
    };

    let mut should_emit = true;
    let (output_preview, is_error) = match result {
        ToolExecutionResult::Completed { raw_outcome, .. } => {
            let preview = match raw_outcome {
                RawToolOutcome::Success { output } => {
                    // Filter password for ask_user_question tool
                    let tool_name = result.tool_name();
                    if tool_name == "ask_user_question" {
                        filter_ask_user_question_output(output)
                    } else {
                        output.clone()
                    }
                }
                RawToolOutcome::Error { message } => message.clone(),
            };
            (preview, false)
        }
        ToolExecutionResult::Suspended { suspend_token, .. } => {
            should_emit = false;
            (format!("suspended:{suspend_token}"), false)
        }
        ToolExecutionResult::Failed {
            execution_error, ..
        } => (execution_error.to_string(), true),
        ToolExecutionResult::Denied { error, .. } => (
            error.as_ref().map(|e| e.to_string()).unwrap_or_default(),
            true,
        ),
    };

    if should_emit {
        let agent_id = agent_id_or_anonymous(ctx.input.agent_id.as_ref());

        // Extract secrets and filter args_preview for bash commands
        let messages = ctx.state.messages.read().clone();
        let secrets = extract_secrets_from_messages(&messages);
        let args_preview = serde_json::to_string_pretty(&result.final_call().input)
            .unwrap_or_else(|_| result.final_call().input.to_string());
        let filtered_args_preview = if result.tool_name() == "bash" {
            filter_bash_args_preview(&args_preview, &secrets)
        } else {
            args_preview
        };

        sink.on_tool_result(
            agent_id,
            &ToolResultEvent {
                call_id: result.call_id().to_string(),
                tool_name: result.tool_name().to_string(),
                output_preview,
                is_error,
                args_preview: filtered_args_preview,
            },
        );
    }
}

/// Filter password in ask_user_question output for display
fn filter_ask_user_question_output(output: &str) -> String {
    // Try to parse as AskUserQuestionOutput and filter display_value
    if let Ok(mut json_value) = serde_json::from_str::<serde_json::Value>(output) {
        if let Some(answers) = json_value.get_mut("answers") {
            if let Some(answers_array) = answers.as_array_mut() {
                for answer in answers_array {
                    // For Text type answers with display_value, use display_value instead of value
                    if let Some(kind) = answer.get("kind") {
                        if kind.as_str() == Some("text") {
                            // Get display_value first
                            let display_value = answer.get("display_value").and_then(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    Some(v.clone())
                                }
                            });

                            // If display_value exists, replace value
                            if let Some(display_val) = display_value {
                                if let Some(obj) = answer.as_object_mut() {
                                    obj["value"] = display_val;
                                    obj.remove("display_value");
                                }
                            }
                        }
                    }
                }
            }
        }
        // Serialize back and take first 200 chars
        if let Ok(filtered_output) = serde_json::to_string(&json_value) {
            return filtered_output.chars().take(200).collect();
        }
    }
    // Fallback: original output (first 200 chars)
    output.chars().take(200).collect()
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

/// Filter password in bash args_preview (command field)
fn filter_bash_args_preview(args_preview: &str, secrets: &[String]) -> String {
    if let Ok(mut json_value) = serde_json::from_str::<serde_json::Value>(args_preview) {
        if let Some(command) = json_value.get("command").and_then(|c| c.as_str()) {
            let filtered_command = filter_secrets_in_text(command, secrets);
            if let Some(obj) = json_value.as_object_mut() {
                obj["command"] = serde_json::Value::String(filtered_command);
            }
        }
        if let Ok(filtered) = serde_json::to_string_pretty(&json_value) {
            return filtered;
        }
    }
    // Fallback: filter entire string
    filter_secrets_in_text(args_preview, secrets)
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
            ctx.state.messages.write().push(ChatMessage::user(reminder));
            ctx.turn.decision = Some(LoopDecision::Continue);
            return;
        }
    }

    ctx.turn.decision = Some(LoopDecision::ReturnComplete);
}

fn should_stop_after_tool_result(ctx: &LoopContext<'_>, result: &ToolExecutionResult) -> bool {
    ctx.input
        .stop_rules
        .iter()
        .any(|rule| stop_rule_matches_tool_result(rule, result))
}

fn stop_rule_matches_tool_result(rule: &LoopStopRule, result: &ToolExecutionResult) -> bool {
    match rule {
        LoopStopRule::AfterSuccessfulTool { tool_name } => {
            result.tool_name() == tool_name
                && matches!(
                    result,
                    ToolExecutionResult::Completed {
                        raw_outcome: RawToolOutcome::Success { .. },
                        ..
                    }
                )
        }
    }
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

    ctx.state.messages.write().push(ChatMessage {
        role: MessageRole::Assistant,
        blocks,
        message_id: None,
        timestamp_ms: now_ms(),
        api_usage_tokens: Some(msg.usage.total_tokens),
        reasoning_content: msg.reasoning_content.clone(),
        estimated_tokens: None,
    });
}

pub fn build_tool_result_message(result: &ToolExecutionResult) -> ChatMessage {
    let (call_id, tool_name, output, is_error) = match result {
        ToolExecutionResult::Completed {
            final_call,
            raw_outcome,
            ..
        } => {
            let (out, err) = match raw_outcome {
                RawToolOutcome::Success { output } => (output.clone(), false),
                RawToolOutcome::Error { message } => (message.clone(), true),
            };
            (
                final_call.call_id.clone(),
                final_call.tool_name.clone(),
                out,
                err,
            )
        }
        ToolExecutionResult::Suspended {
            final_call,
            suspend_token,
            ..
        } => (
            final_call.call_id.clone(),
            final_call.tool_name.clone(),
            format!("suspended:{suspend_token}"),
            false,
        ),
        ToolExecutionResult::Failed {
            final_call,
            execution_error,
            ..
        } => (
            final_call.call_id.clone(),
            final_call.tool_name.clone(),
            execution_error.to_string(),
            true,
        ),
        ToolExecutionResult::Denied {
            final_call, error, ..
        } => (
            final_call.call_id.clone(),
            final_call.tool_name.clone(),
            format!(
                "denied: {}",
                error.as_ref().map(|e| e.to_string()).unwrap_or_default()
            ),
            true,
        ),
    };

    ChatMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            call_id,
            tool_name,
            output,
            is_error,
        }],
        message_id: None,
        timestamp_ms: now_ms(),
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    }
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
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    use agent_contracts::context::budget::TokenBudgetPolicy;
    use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolRegistry, ToolSpecView};
    use agent_contracts::{
        CompressionPipeline, LlmProvider, PromptBuilder, ProviderCapabilities, RuntimeView,
        SkillRegistry,
    };
    use agent_llm::LlmRequestExt;
    use agent_types::common::ids::{ToolId, ToolName};
    use agent_types::context::budget::BudgetError;
    use agent_types::context::prompt::{PromptBuildError, PromptBuildResult};
    use agent_types::context::{FeatureFlags, TokenBudgetConfig};
    use agent_types::events::LoopEndSummary;
    use agent_types::tool::execution_types::{ToolExecutionError, ToolExecutorOutput};
    use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
    use agent_types::tool::FinalToolCall;
    use agent_types::{
        AssistantMessage, LlmError, LlmRequest, LlmResponse, StopReason, StreamChunk, ToolUseBlock,
        Usage,
    };
    use async_trait::async_trait;
    use llm_client::LlmProviderWrapper;
    use tool::EmptyToolRegistry;

    use crate::runtime_support::{EmptySkillRegistry, NoopRuntimeView};

    #[test]
    fn transient_backoff_grows_exponentially_without_retry_after() {
        assert_eq!(transient_backoff(0, 0), Duration::from_millis(4_000));
        assert_eq!(transient_backoff(1, 0), Duration::from_millis(8_000));
        assert_eq!(transient_backoff(2, 0), Duration::from_millis(16_000));
        assert_eq!(transient_backoff(3, 0), Duration::from_millis(32_000));
    }

    #[test]
    fn transient_backoff_is_clamped_to_the_ceiling() {
        assert_eq!(transient_backoff(10, 0), Duration::from_millis(60_000));
        assert_eq!(
            transient_backoff(u32::MAX, 0),
            Duration::from_millis(60_000)
        );
    }

    #[test]
    fn transient_backoff_honors_retry_after_within_the_ceiling() {
        assert_eq!(transient_backoff(3, 5_000), Duration::from_millis(5_000));
        assert_eq!(transient_backoff(0, 120_000), Duration::from_millis(60_000));
    }

    #[test]
    fn is_transient_retries_network_and_throttle_errors_only() {
        assert!(is_transient(&LlmError::HttpError(
            "error sending request for url".into()
        )));
        assert!(is_transient(&LlmError::Timeout));
        assert!(is_transient(&LlmError::RateLimited {
            retry_after_ms: 0,
            message: String::new(),
        }));

        assert!(!is_transient(&LlmError::AuthError {
            message: String::new(),
        }));
        assert!(!is_transient(&LlmError::ApiError("HTTP 400".into())));
        assert!(!is_transient(&LlmError::ContextLengthExceeded {
            message: String::new(),
        }));
    }

    fn assistant_with_tool_calls(calls: &[(&str, &str)]) -> AssistantMessage {
        AssistantMessage {
            text: None,
            reasoning_content: None,
            tool_calls: calls
                .iter()
                .map(|(call_id, tool_name)| ToolUseBlock {
                    call_id: call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    input: serde_json::json!({}),
                })
                .collect(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
        }
    }

    #[test]
    fn synthesize_missing_call_ids_fills_only_empty_ids_uniquely() {
        let mut msg = assistant_with_tool_calls(&[
            ("", "grep"),          // missing id → synthesized
            ("   ", "bash"),       // whitespace-only id → treated as missing
            ("call_real", "edit"), // provider-supplied id → preserved
        ]);

        synthesize_missing_call_ids(&mut msg, 3);

        assert_eq!(msg.tool_calls[0].call_id, "call_3_0");
        assert_eq!(msg.tool_calls[1].call_id, "call_3_1");
        assert_eq!(msg.tool_calls[2].call_id, "call_real");
        assert!(msg.tool_calls.iter().all(is_valid_tool_call));
        assert_ne!(msg.tool_calls[0].call_id, msg.tool_calls[1].call_id);

        let mut next = assistant_with_tool_calls(&[("", "grep")]);
        synthesize_missing_call_ids(&mut next, 4);
        assert_eq!(next.tool_calls[0].call_id, "call_4_0");
        assert_ne!(next.tool_calls[0].call_id, msg.tool_calls[0].call_id);
    }

    #[test]
    fn synthesize_missing_call_ids_does_not_rescue_empty_tool_name() {
        let mut msg = assistant_with_tool_calls(&[("", "")]);
        synthesize_missing_call_ids(&mut msg, 0);
        assert_eq!(msg.tool_calls[0].call_id, "call_0_0");
        assert!(!is_valid_tool_call(&msg.tool_calls[0]));
    }

    #[test]
    fn compression_trigger_as_str() {
        assert_eq!(CompressionTrigger::Automatic.as_str(), "automatic");
        assert_eq!(
            CompressionTrigger::ContextLimitRetry.as_str(),
            "context_limit_retry"
        );
        assert_eq!(
            CompressionTrigger::PreCheckExceeded.as_str(),
            "pre_check_exceeded"
        );
    }

    #[test]
    fn compression_trigger_is_forced() {
        assert!(!CompressionTrigger::Automatic.is_forced());
        assert!(CompressionTrigger::ContextLimitRetry.is_forced());
        assert!(CompressionTrigger::PreCheckExceeded.is_forced());
    }

    mod token_budget_tests {
        use super::*;
        use crate::token_estimator::TokenEstimator;
        use agent_types::MessageRole;

        #[test]
        fn test_estimator_basic_calculation() {
            let estimator = TokenEstimator::new();
            let messages = vec![
                ChatMessage::text(MessageRole::User, "Hello world", 0),
                ChatMessage::text(MessageRole::Assistant, "Hi there", 0),
            ];

            let estimated =
                estimator.estimate_input_tokens("You are a helpful assistant", 0, &messages);

            assert!(estimated > 0);
        }

        #[test]
        fn test_budget_calculation_logic() {
            let config = TokenBudgetConfig {
                total_budget: 10000,
                reserved_for_output: 1000,
                reserved_for_system: 500,
                hard_limit_ratio: 0.8,
            };

            let available_for_input = config
                .total_budget
                .saturating_sub(config.reserved_for_output)
                .saturating_sub(config.reserved_for_system);

            assert_eq!(available_for_input, 8500);
        }

        #[test]
        fn test_budget_edge_case_zero_available() {
            let config = TokenBudgetConfig {
                total_budget: 1000,
                reserved_for_output: 1000,
                reserved_for_system: 500,
                hard_limit_ratio: 0.8,
            };

            let available_for_input = config
                .total_budget
                .saturating_sub(config.reserved_for_output)
                .saturating_sub(config.reserved_for_system);

            assert_eq!(available_for_input, 0);
        }

        #[test]
        fn test_budget_edge_case_over_allocation() {
            let config = TokenBudgetConfig {
                total_budget: 1000,
                reserved_for_output: 800,
                reserved_for_system: 400,
                hard_limit_ratio: 0.8,
            };

            let available_for_input = config
                .total_budget
                .saturating_sub(config.reserved_for_output)
                .saturating_sub(config.reserved_for_system);

            assert_eq!(available_for_input, 0);

            let is_invalid =
                config.reserved_for_output + config.reserved_for_system >= config.total_budget;
            assert!(is_invalid);
        }
    }

    struct StreamingTestProvider {
        capabilities: ProviderCapabilities,
    }

    impl StreamingTestProvider {
        fn new() -> Self {
            Self {
                capabilities: ProviderCapabilities {
                    supports_streaming: true,
                    supports_tool_calls: false,
                    supports_json_mode: false,
                    max_context_window: 4096,
                    model_name: "streaming-test".to_string(),
                },
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StreamingTestProvider {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            panic!("streaming path should use complete_stream instead of complete");
        }

        async fn complete_stream(
            &self,
            _request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            on_chunk(StreamChunk {
                delta_text: Some("Hello".to_string()),
                delta_reasoning: None,
                delta_tool_call: None,
            });
            on_chunk(StreamChunk {
                delta_text: Some(" world".to_string()),
                delta_reasoning: None,
                delta_tool_call: None,
            });

            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some("Hello world".to_string()),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                    usage: Usage {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                        total_tokens: 5,
                        cached_tokens: 0,
                    },
                    stop_reason: StopReason::EndTurn,
                },
                kv_cache_chunk_hashes: vec![],
            })
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }

    struct SequentialUsageProvider {
        capabilities: ProviderCapabilities,
        call_count: Arc<StdMutex<usize>>,
    }

    impl SequentialUsageProvider {
        fn new(call_count: Arc<StdMutex<usize>>) -> Self {
            Self {
                capabilities: ProviderCapabilities {
                    supports_streaming: true,
                    supports_tool_calls: false,
                    supports_json_mode: false,
                    max_context_window: 4096,
                    model_name: "sequential-usage-test".to_string(),
                },
                call_count,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for SequentialUsageProvider {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            panic!("streaming path should use complete_stream instead of complete");
        }

        async fn complete_stream(
            &self,
            _request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            let call_number = {
                let mut count = self
                    .call_count
                    .lock()
                    .expect("provider call count mutex should not be poisoned");
                *count += 1;
                *count
            };

            let (text, usage) = if call_number == 1 {
                (
                    "first turn".to_string(),
                    Usage {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                        total_tokens: 5,
                        cached_tokens: 0,
                    },
                )
            } else {
                (
                    "second turn".to_string(),
                    Usage {
                        prompt_tokens: 7,
                        completion_tokens: 1,
                        total_tokens: 8,
                        cached_tokens: 0,
                    },
                )
            };

            on_chunk(StreamChunk {
                delta_text: Some(text.clone()),
                delta_reasoning: None,
                delta_tool_call: None,
            });

            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some(text),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                    usage,
                    stop_reason: StopReason::EndTurn,
                },
                kv_cache_chunk_hashes: vec![],
            })
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }

    struct FixedPromptBuilder;

    #[async_trait]
    impl PromptBuilder for FixedPromptBuilder {
        async fn build(
            &self,
            input: PromptBuildInput,
        ) -> Result<PromptBuildResult, PromptBuildError> {
            Ok(PromptBuildResult {
                request: LlmRequest::new(input.messages),
                estimated_input_tokens: 0,
                system_parts: Vec::new(),
            })
        }
    }

    struct FixedBudgetPolicy {
        config: TokenBudgetConfig,
    }

    impl FixedBudgetPolicy {
        fn new(config: TokenBudgetConfig) -> Self {
            Self { config }
        }
    }

    impl TokenBudgetPolicy for FixedBudgetPolicy {
        fn total_budget(&self) -> usize {
            self.config.total_budget
        }

        fn reserved_for_output(&self) -> usize {
            self.config.reserved_for_output
        }

        fn reserved_for_system(&self) -> usize {
            self.config.reserved_for_system
        }

        fn hard_limit_ratio(&self) -> f64 {
            self.config.hard_limit_ratio
        }

        fn validate(&self) -> Result<(), BudgetError> {
            Ok(())
        }

        fn available_budget(&self) -> Result<usize, BudgetError> {
            Ok(self
                .config
                .total_budget
                .saturating_sub(self.config.reserved_for_output)
                .saturating_sub(self.config.reserved_for_system))
        }

        fn history_limit(&self) -> Result<usize, BudgetError> {
            self.available_budget()
        }
    }

    struct VisibleToolSpec {
        id: ToolId,
        name: ToolName,
        description: String,
        input_schema: InputSchemaRef,
        output_contract: OutputContract,
        effect_profile: EffectProfile,
    }

    impl ToolSpecView for VisibleToolSpec {
        fn id(&self) -> &ToolId {
            &self.id
        }

        fn name(&self) -> &ToolName {
            &self.name
        }

        fn description(&self) -> &str {
            &self.description
        }

        fn input_schema(&self) -> &InputSchemaRef {
            &self.input_schema
        }

        fn output_contract(&self) -> &OutputContract {
            &self.output_contract
        }

        fn effect_profile(&self) -> &EffectProfile {
            &self.effect_profile
        }
    }

    fn dummy_visible_tools() -> Vec<Arc<dyn ToolSpecView>> {
        vec![Arc::new(VisibleToolSpec {
            id: ToolId("tool.bash".to_string()),
            name: ToolName("bash".to_string()),
            description: "Execute a shell command".to_string(),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"]
                }),
            },
            output_contract: OutputContract {
                description: "Command output".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: false,
                network_access: false,
                side_effects: true,
            },
        })]
    }

    fn test_runtime(provider: Arc<LlmProviderWrapper>) -> AgentRuntime {
        test_runtime_with_max_turns(provider, 4)
    }

    fn test_runtime_with_max_turns(
        provider: Arc<LlmProviderWrapper>,
        max_turns: u32,
    ) -> AgentRuntime {
        test_runtime_with_registry(provider, max_turns, Arc::new(EmptyToolRegistry::new()))
    }

    fn test_runtime_with_registry(
        provider: Arc<LlmProviderWrapper>,
        max_turns: u32,
        tool_registry: Arc<dyn ToolRegistry>,
    ) -> AgentRuntime {
        let prompt_builder: Arc<dyn PromptBuilder> = Arc::new(FixedPromptBuilder);
        let compression_pipeline: Arc<dyn CompressionPipeline> =
            Arc::new(compact::PassthroughCompressionPipeline::new());
        let skill_registry: Arc<dyn SkillRegistry> = Arc::new(EmptySkillRegistry::new());
        let budget_config = TokenBudgetConfig {
            total_budget: 4096,
            reserved_for_output: 512,
            reserved_for_system: 256,
            hard_limit_ratio: 1.0,
        };
        let budget_policy: Arc<dyn TokenBudgetPolicy> =
            Arc::new(FixedBudgetPolicy::new(budget_config.clone()));

        AgentRuntime::builder()
            .llm_provider(provider)
            .compression_pipeline(compression_pipeline)
            .prompt_builder(prompt_builder)
            .system_prompt("You are a coding agent.")
            .tool_registry(tool_registry)
            .skill_registry(skill_registry)
            .feature_flags(FeatureFlags::default())
            .max_turns(max_turns)
            .token_budget_config(budget_config)
            .token_budget_policy(budget_policy)
            .build()
            .expect("test runtime should build")
    }

    #[derive(Default)]
    struct RecordingLoopEventSink {
        assistant_messages: Mutex<Vec<String>>,
    }

    impl RecordingLoopEventSink {
        fn take_assistant_messages(&self) -> Vec<String> {
            self.assistant_messages
                .lock()
                .expect("assistant message recorder mutex should not be poisoned")
                .clone()
        }
    }

    impl LoopEventSink for RecordingLoopEventSink {
        fn on_turn_start(&self, _agent_id: &AgentId, _turn: u32) {}

        fn on_assistant_message(&self, _agent_id: &AgentId, text: &str) {
            self.assistant_messages
                .lock()
                .expect("assistant message recorder mutex should not be poisoned")
                .push(text.to_string());
        }

        fn on_tool_result(&self, _agent_id: &AgentId, _event: &ToolResultEvent) {}

        fn on_loop_end(&self, _agent_id: &AgentId, _summary: &LoopEndSummary) {}
    }

    #[tokio::test]
    async fn run_agent_loop_emits_streaming_assistant_snapshots() {
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(StreamingTestProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime(provider);
        let sink = Arc::new(RecordingLoopEventSink::default());
        let input = AgentLoopInput::new("hello")
            .with_agent_id(AgentId("test-agent".to_string()))
            .with_event_sink(sink.clone());
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

        let outcome = run_agent_loop(&runtime, &mut loop_state, input)
            .await
            .expect("streaming test loop should succeed");

        assert!(matches!(
            outcome,
            LoopRunResult::Complete(AgentOutcome::Complete { .. })
        ));
        assert_eq!(
            sink.take_assistant_messages(),
            vec!["Hello".to_string(), "Hello world".to_string()]
        );
        assert_eq!(loop_state.token_usage.total_tokens, 5);
        assert_eq!(
            loop_state
                .messages
                .read()
                .last()
                .and_then(ChatMessage::text_content),
            Some("Hello world")
        );
    }

    #[tokio::test]
    async fn run_agent_loop_applies_max_turns_per_run_not_session_total() {
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(StreamingTestProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime_with_max_turns(provider, 2);
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());
        loop_state.turn_count = 10;

        let outcome = run_agent_loop(
            &runtime,
            &mut loop_state,
            AgentLoopInput::new("revise plan"),
        )
        .await
        .expect("loop should complete despite prior session turns");

        assert!(matches!(
            outcome,
            LoopRunResult::Complete(AgentOutcome::Complete { .. })
        ));
        assert_eq!(loop_state.turn_count, 11);
    }

    #[tokio::test]
    async fn run_agent_loop_surfaces_max_turns_when_final_turn_yields_text() {
        // Regression: with `max_turns = 1`, `build_messages` withholds tools
        // on turn 1 (the final turn) so the model replies with text only.
        // `decide` must still return `MaxTurnsReached` — not `Complete` — so
        // the `*.Session.lifecycle.state` hook payload carries
        // `outcome = "max_turns_reached"`. Previously the `MaxTurnsReached`
        // branch was only reachable when the assistant emitted tool calls,
        // which the no-tools final turn makes impossible.
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(StreamingTestProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime_with_max_turns(provider, 1);
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

        let outcome = run_agent_loop(&runtime, &mut loop_state, AgentLoopInput::new("hi"))
            .await
            .expect("loop should terminate at max_turns");

        match outcome {
            LoopRunResult::Complete(AgentOutcome::MaxTurnsReached { partial_reply, .. }) => {
                assert_eq!(partial_reply.as_deref(), Some("Hello world"));
                assert_eq!(loop_state.turn_count, 1);
            }
            _ => panic!("expected MaxTurnsReached, got a different outcome"),
        }
    }

    #[test]
    fn loop_stop_rule_matches_only_configured_successful_tool() {
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(StreamingTestProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime(provider);
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());
        let input =
            AgentLoopInput::new("plan").with_stop_rules([LoopStopRule::AfterSuccessfulTool {
                tool_name: "todo_write".to_string(),
            }]);
        let ctx = LoopContext {
            snapshot: runtime.snapshot(),
            state: &mut loop_state,
            input,
            turn: TurnState::new(1),
        };
        let result = ToolExecutionResult::Completed {
            final_call: agent_types::tool::FinalToolCall {
                call_id: "call_1".to_string(),
                tool_name: "todo_write".to_string(),
                input: serde_json::json!({}),
            },
            raw_outcome: RawToolOutcome::Success {
                output: "ok".to_string(),
            },
            pre_hook_results: Vec::new(),
            post_hook_results: Vec::new(),
        };
        let failed_result = ToolExecutionResult::Completed {
            final_call: agent_types::tool::FinalToolCall {
                call_id: "call_2".to_string(),
                tool_name: "todo_write".to_string(),
                input: serde_json::json!({}),
            },
            raw_outcome: RawToolOutcome::Error {
                message: "bad input".to_string(),
            },
            pre_hook_results: Vec::new(),
            post_hook_results: Vec::new(),
        };

        assert!(should_stop_after_tool_result(&ctx, &result));
        assert!(!should_stop_after_tool_result(&ctx, &failed_result));
    }

    #[tokio::test]
    async fn run_agent_loop_overwrites_token_usage_with_current_turn_usage() {
        let call_count = Arc::new(StdMutex::new(0));
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(SequentialUsageProvider::new(call_count)),
            None,
            None,
        ));
        let runtime = test_runtime(provider);
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

        run_agent_loop(&runtime, &mut loop_state, AgentLoopInput::new("first"))
            .await
            .expect("first loop run should succeed");
        assert_eq!(loop_state.token_usage.prompt_tokens, 3);
        assert_eq!(loop_state.token_usage.completion_tokens, 2);
        assert_eq!(loop_state.token_usage.total_tokens, 5);

        let outcome = run_agent_loop(&runtime, &mut loop_state, AgentLoopInput::new("second"))
            .await
            .expect("second loop run should succeed");

        assert_eq!(loop_state.token_usage.prompt_tokens, 7);
        assert_eq!(loop_state.token_usage.completion_tokens, 1);
        assert_eq!(loop_state.token_usage.total_tokens, 8);

        match outcome {
            LoopRunResult::Complete(AgentOutcome::Complete { token_usage, .. }) => {
                assert_eq!(token_usage.prompt_tokens, 7);
                assert_eq!(token_usage.completion_tokens, 1);
                assert_eq!(token_usage.total_tokens, 8);
            }
            _ => panic!("unexpected outcome"),
        }
    }

    /// Streams an empty-`call_id` tool call on the first turn, then a plain
    /// completion so the loop terminates after the call is executed.
    struct EmptyCallIdToolCallProvider {
        capabilities: ProviderCapabilities,
        calls: Arc<StdMutex<usize>>,
    }

    impl EmptyCallIdToolCallProvider {
        fn new() -> Self {
            Self {
                capabilities: ProviderCapabilities {
                    supports_streaming: true,
                    supports_tool_calls: true,
                    supports_json_mode: false,
                    max_context_window: 4096,
                    model_name: "empty-call-id-test".to_string(),
                },
                calls: Arc::new(StdMutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for EmptyCallIdToolCallProvider {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            panic!("streaming path should use complete_stream instead of complete");
        }

        async fn complete_stream(
            &self,
            _request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            let call_number = {
                let mut calls = self.calls.lock().expect("call counter mutex poisoned");
                *calls += 1;
                *calls
            };

            on_chunk(StreamChunk {
                delta_text: Some("trying to use a tool".to_string()),
                delta_reasoning: None,
                delta_tool_call: None,
            });

            if call_number >= 2 {
                return Ok(LlmResponse {
                    message: AssistantMessage {
                        text: Some("done".to_string()),
                        reasoning_content: None,
                        tool_calls: Vec::new(),
                        usage: Usage {
                            prompt_tokens: 10,
                            completion_tokens: 2,
                            total_tokens: 12,
                            cached_tokens: 0,
                        },
                        stop_reason: StopReason::EndTurn,
                    },
                    kv_cache_chunk_hashes: vec![],
                });
            }

            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some("trying to use a tool".to_string()),
                    reasoning_content: None,
                    tool_calls: vec![ToolUseBlock {
                        call_id: String::new(),
                        tool_name: "bash".to_string(),
                        input: serde_json::json!({"command": "date"}),
                    }],
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                        cached_tokens: 0,
                    },
                    stop_reason: StopReason::ToolUse,
                },
                kv_cache_chunk_hashes: vec![],
            })
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }

    #[tokio::test]
    async fn run_agent_loop_synthesizes_missing_call_id_and_executes_tool() {
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(EmptyCallIdToolCallProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime(provider);
        let input = AgentLoopInput::new("现在几点")
            .with_agent_id(AgentId("test-agent".to_string()))
            .with_visible_tools(dummy_visible_tools())
            .with_runtime_view(Arc::new(NoopRuntimeView::new()));
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

        let outcome = run_agent_loop(&runtime, &mut loop_state, input)
            .await
            .expect("loop should execute the call after synthesizing its id");

        assert!(matches!(
            outcome,
            LoopRunResult::Complete(AgentOutcome::Complete { .. })
        ));
        // turn 1: synthesize + run the tool; turn 2: model stops and the loop
        // completes without injecting a checklist that could replace the answer.
        assert_eq!(loop_state.turn_count, 2);

        let messages = loop_state.messages.read();
        assert!(
            messages
                .iter()
                .filter_map(ChatMessage::text_content)
                .all(|text| !text.contains("You are about to finish")),
            "completion should not inject a checklist as a new user request"
        );
        let tool_use = messages.iter().find_map(|m| {
            m.blocks.iter().find_map(|b| match b {
                ContentBlock::ToolUse {
                    call_id, tool_name, ..
                } => Some((call_id.clone(), tool_name.clone())),
                _ => None,
            })
        });
        assert_eq!(
            tool_use,
            Some(("call_0_0".to_string(), "bash".to_string())),
            "empty call_id should be synthesized to a stable turn-scoped id and preserved"
        );
        let paired = messages.iter().any(|m| {
            matches!(m.role, MessageRole::Tool)
                && m.blocks.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::ToolResult { call_id, .. } if call_id == "call_0_0"
                    )
                })
        });
        assert!(
            paired,
            "synthesized tool_use must pair with a tool_result on the same id"
        );
    }

    #[test]
    fn test_filter_ask_user_question_output() {
        // Test with display_value
        let input_with_display = json!({
            "answers": [{
                "kind": "text",
                "prompt": "Enter password",
                "value": "real_password_123",
                "display_value": "<SECRET>"
            }]
        })
        .to_string();

        let filtered = filter_ask_user_question_output(&input_with_display);

        // Should replace value with display_value
        let filtered_json: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        assert_eq!(filtered_json["answers"][0]["value"], "<SECRET>");
        assert!(filtered_json["answers"][0].get("display_value").is_none());

        // Test without display_value
        let input_without_display = json!({
            "answers": [{
                "kind": "text",
                "prompt": "Enter name",
                "value": "John"
            }]
        })
        .to_string();

        let filtered2 = filter_ask_user_question_output(&input_without_display);
        let filtered2_json: serde_json::Value = serde_json::from_str(&filtered2).unwrap();
        assert_eq!(filtered2_json["answers"][0]["value"], "John");

        // Test with non-text type
        let input_choice = json!({
            "answers": [{
                "kind": "choice",
                "prompt": "Select option",
                "value": "option1"
            }]
        })
        .to_string();

        let filtered3 = filter_ask_user_question_output(&input_choice);
        let filtered3_json: serde_json::Value = serde_json::from_str(&filtered3).unwrap();
        assert_eq!(filtered3_json["answers"][0]["value"], "option1");

        // Test with invalid JSON
        let invalid = "not a json";
        let filtered4 = filter_ask_user_question_output(invalid);
        assert_eq!(filtered4, "not a json");
    }

    #[test]
    fn test_extract_secrets_from_messages() {
        // Test 1: Only extract secrets with display_value (is_secret=true)
        let message_with_secret = ChatMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call_1".to_string(),
                tool_name: "ask_user_question".to_string(),
                output: json!({
                    "answers": [{
                        "kind": "text",
                        "prompt": "Password",
                        "value": "secret123",
                        "display_value": "<SECRET>"
                    }]
                })
                .to_string(),
                is_error: false,
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        };

        let message_with_normal_text = ChatMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call_2".to_string(),
                tool_name: "ask_user_question".to_string(),
                output: json!({
                    "answers": [{
                        "kind": "text",
                        "prompt": "Username",
                        "value": "john"
                    }]
                })
                .to_string(),
                is_error: false,
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        };

        let messages = vec![message_with_secret, message_with_normal_text];
        let secrets = extract_secrets_from_messages(&messages);

        // Should only extract the secret with display_value
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0], "secret123");
        assert!(!secrets.contains(&"john".to_string()));

        // Test 2: Multiple secrets in one answer
        let message_multiple = ChatMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call_3".to_string(),
                tool_name: "ask_user_question".to_string(),
                output: json!({
                    "answers": [
                        {
                            "kind": "text",
                            "prompt": "Username",
                            "value": "admin"
                        },
                        {
                            "kind": "text",
                            "prompt": "Password",
                            "value": "pass123",
                            "display_value": "<SECRET>"
                        }
                    ]
                })
                .to_string(),
                is_error: false,
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        };

        let secrets2 = extract_secrets_from_messages(&vec![message_multiple]);
        assert_eq!(secrets2.len(), 1);
        assert_eq!(secrets2[0], "pass123");
        assert!(!secrets2.contains(&"admin".to_string()));

        // Test 3: Non-ask_user_question tool results should not be extracted
        let message_other_tool = ChatMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call_4".to_string(),
                tool_name: "bash".to_string(),
                output: "some output with password123".to_string(),
                is_error: false,
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        };

        let secrets3 = extract_secrets_from_messages(&vec![message_other_tool]);
        assert_eq!(secrets3.len(), 0);
    }

    #[test]
    fn is_parallel_safe_allows_pure_readers_only() {
        let reader = EffectProfile {
            reads_filesystem: true,
            writes_filesystem: false,
            network_access: false,
            side_effects: false,
        };
        let network_reader = EffectProfile {
            reads_filesystem: false,
            writes_filesystem: false,
            network_access: true,
            side_effects: false,
        };
        assert!(is_parallel_safe(&reader));
        assert!(is_parallel_safe(&network_reader));
    }

    #[test]
    fn is_parallel_safe_serializes_writers_side_effects_and_interactive() {
        let writer = EffectProfile {
            reads_filesystem: true,
            writes_filesystem: true,
            network_access: false,
            side_effects: false,
        };
        let side_effecting = EffectProfile {
            reads_filesystem: true,
            writes_filesystem: true,
            network_access: false,
            side_effects: true,
        };
        let stateful = EffectProfile {
            reads_filesystem: false,
            writes_filesystem: false,
            network_access: false,
            side_effects: true,
        };
        let interactive = EffectProfile::default();
        assert!(!is_parallel_safe(&writer));
        assert!(!is_parallel_safe(&side_effecting));
        assert!(!is_parallel_safe(&stateful));
        assert!(
            !is_parallel_safe(&interactive),
            "an interactive prompt declares no read/network and must serialize"
        );
    }

    #[tokio::test]
    async fn conversational_run_with_visible_tools_completes_in_one_turn() {
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(StreamingTestProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime(provider);
        let input = AgentLoopInput::new("explain how this code works")
            .with_agent_id(AgentId("test-agent".to_string()))
            .with_visible_tools(dummy_visible_tools())
            .with_runtime_view(Arc::new(NoopRuntimeView::new()));
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

        let outcome = run_agent_loop(&runtime, &mut loop_state, input)
            .await
            .expect("conversational loop should complete");

        assert!(matches!(
            outcome,
            LoopRunResult::Complete(AgentOutcome::Complete { .. })
        ));
        assert!(
            !loop_state.tool_executed,
            "no tool ran, so the run is conversational"
        );
        assert_eq!(loop_state.turn_count, 1);
    }

    struct AlwaysSucceedsExecutor {
        spec: Arc<VisibleToolSpec>,
    }

    #[async_trait]
    impl ToolExecutor for AlwaysSucceedsExecutor {
        fn spec(&self) -> &dyn ToolSpecView {
            self.spec.as_ref()
        }

        async fn invoke(
            &self,
            call: &FinalToolCall,
            _runtime: &dyn RuntimeView,
        ) -> Result<ToolExecutorOutput, ToolExecutionError> {
            Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success {
                    output: format!("ran {}", call.call_id),
                },
            })
        }
    }

    struct SingleToolRegistry {
        spec: Arc<VisibleToolSpec>,
        executor: Arc<dyn ToolExecutor>,
    }

    impl SingleToolRegistry {
        fn new() -> Self {
            let spec = Arc::new(VisibleToolSpec {
                id: ToolId("tool.peek".to_string()),
                name: ToolName("peek".to_string()),
                description: "Read-only peek".to_string(),
                input_schema: InputSchemaRef {
                    schema: serde_json::json!({"type": "object"}),
                },
                output_contract: OutputContract {
                    description: "peeked".to_string(),
                },
                effect_profile: EffectProfile {
                    reads_filesystem: true,
                    writes_filesystem: false,
                    network_access: false,
                    side_effects: false,
                },
            });
            let executor: Arc<dyn ToolExecutor> = Arc::new(AlwaysSucceedsExecutor {
                spec: Arc::clone(&spec),
            });
            Self { spec, executor }
        }

        fn visible(&self) -> Vec<Arc<dyn ToolSpecView>> {
            vec![Arc::clone(&self.spec) as Arc<dyn ToolSpecView>]
        }
    }

    impl ToolRegistry for SingleToolRegistry {
        fn get_executor(&self, id: &ToolId) -> Option<Arc<dyn ToolExecutor>> {
            (id == self.spec.id()).then(|| Arc::clone(&self.executor))
        }

        fn get_spec(&self, id: &ToolId) -> Option<&dyn ToolSpecView> {
            (id == self.spec.id()).then(|| self.spec.as_ref() as &dyn ToolSpecView)
        }

        fn list_specs(&self) -> Vec<&dyn ToolSpecView> {
            vec![self.spec.as_ref()]
        }

        fn filter_for(&self, _agent_id: &AgentId) -> Box<dyn ToolFilter> {
            tool_filter_from_specs(&self.visible(), self)
        }
    }

    struct TwoToolCallProvider {
        capabilities: ProviderCapabilities,
        calls: Arc<StdMutex<usize>>,
    }

    impl TwoToolCallProvider {
        fn new() -> Self {
            Self {
                capabilities: ProviderCapabilities {
                    supports_streaming: true,
                    supports_tool_calls: true,
                    supports_json_mode: false,
                    max_context_window: 4096,
                    model_name: "two-tool-call-test".to_string(),
                },
                calls: Arc::new(StdMutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for TwoToolCallProvider {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            panic!("streaming path should use complete_stream instead of complete");
        }

        async fn complete_stream(
            &self,
            _request: &LlmRequest,
            _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            let call_number = {
                let mut calls = self.calls.lock().expect("call counter mutex poisoned");
                *calls += 1;
                *calls
            };

            if call_number == 1 {
                return Ok(LlmResponse {
                    message: AssistantMessage {
                        text: Some("calling tools".to_string()),
                        reasoning_content: None,
                        tool_calls: vec![
                            ToolUseBlock {
                                call_id: "call_a".to_string(),
                                tool_name: "peek".to_string(),
                                input: serde_json::json!({}),
                            },
                            ToolUseBlock {
                                call_id: "call_b".to_string(),
                                tool_name: "peek".to_string(),
                                input: serde_json::json!({}),
                            },
                        ],
                        usage: Usage {
                            cached_tokens: 0,
                            prompt_tokens: 5,
                            completion_tokens: 3,
                            total_tokens: 8,
                        },
                        stop_reason: StopReason::ToolUse,
                    },
                    kv_cache_chunk_hashes: vec![],
                });
            }

            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some("done".to_string()),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                    usage: Usage {
                        cached_tokens: 0,
                        prompt_tokens: 5,
                        completion_tokens: 1,
                        total_tokens: 6,
                    },
                    stop_reason: StopReason::EndTurn,
                },
                kv_cache_chunk_hashes: vec![],
            })
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }

    #[tokio::test]
    async fn mid_batch_stop_still_records_every_executed_result() {
        let registry = Arc::new(SingleToolRegistry::new());
        let visible = registry.visible();
        let provider = Arc::new(LlmProviderWrapper::new(
            Arc::new(TwoToolCallProvider::new()),
            None,
            None,
        ));
        let runtime = test_runtime_with_registry(provider, 4, registry);
        let input = AgentLoopInput::new("go")
            .with_agent_id(AgentId("test-agent".to_string()))
            .with_visible_tools(visible)
            .with_runtime_view(Arc::new(NoopRuntimeView::new()))
            .with_stop_rules([LoopStopRule::AfterSuccessfulTool {
                tool_name: "peek".to_string(),
            }]);
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

        let outcome = run_agent_loop(&runtime, &mut loop_state, input)
            .await
            .expect("loop should complete via the stop rule");

        assert!(matches!(
            outcome,
            LoopRunResult::Complete(AgentOutcome::Complete { .. })
        ));
        assert_eq!(loop_state.turn_count, 1);

        let messages = loop_state.messages.read();
        let tool_use_ids: Vec<String> = messages
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
        let tool_result_ids: Vec<String> = messages
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(
            tool_use_ids,
            vec!["call_a".to_string(), "call_b".to_string()],
            "both tool calls should be in history"
        );
        assert_eq!(
            tool_result_ids,
            vec!["call_a".to_string(), "call_b".to_string()],
            "every executed tool_use must keep its paired tool_result even when an \
             earlier call in the batch triggered the stop rule"
        );
    }

    /// Microbenchmark comparing old (full-text clone) vs new (delta) streaming paths.
    ///
    /// Run with `--nocapture` to see per-chunk timing:
    ///   cargo test stream_chunk_bench -- --nocapture
    #[test]
    fn stream_chunk_bench() {
        use std::sync::Mutex;
        use std::time::Instant;

        // --- Old path sink: default supports_message_delta() = false ---
        #[derive(Default)]
        struct OldPathSink {
            last_text: Mutex<String>,
            last_reasoning: Mutex<String>,
        }
        impl LoopEventSink for OldPathSink {
            fn on_turn_start(&self, _: &AgentId, _: u32) {}
            fn on_assistant_message(&self, _: &AgentId, text: &str) {
                *self.last_text.lock().unwrap() = text.to_string();
            }
            fn on_assistant_reasoning(&self, _: &AgentId, text: &str) {
                *self.last_reasoning.lock().unwrap() = text.to_string();
            }
            fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
            fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
        }

        // --- New path sink: supports_message_delta() = true ---
        #[derive(Default)]
        struct NewPathSink {
            accumulated: Mutex<String>,
        }
        impl LoopEventSink for NewPathSink {
            fn on_turn_start(&self, _: &AgentId, _: u32) {}
            fn on_assistant_message(&self, _: &AgentId, text: &str) {
                // Old path fallback: used when delta support unchecked
                *self.accumulated.lock().unwrap() = text.to_string();
            }
            fn on_assistant_message_delta(&self, _: &AgentId, delta: &str) {
                self.accumulated.lock().unwrap().push_str(delta);
            }
            fn on_assistant_reasoning(&self, _: &AgentId, text: &str) {
                *self.accumulated.lock().unwrap() = text.to_string();
            }
            fn on_assistant_reasoning_delta(&self, _: &AgentId, delta: &str) {
                self.accumulated.lock().unwrap().push_str(delta);
            }
            fn supports_message_delta(&self) -> bool {
                true
            }
            fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
            fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
        }

        // Generate test chunks: 500 chunks × 80 bytes = 40K response
        let chunk_base = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. ";
        let num_chunks = 500;
        let chunks: Vec<StreamChunk> = (0..num_chunks)
            .map(|i| {
                let offset = (i * 7) % (chunk_base.len() - 40);
                let text = &chunk_base[offset..offset + 40];
                StreamChunk {
                    delta_text: Some(text.to_string()),
                    delta_reasoning: None,
                    delta_tool_call: None,
                }
            })
            .collect();
        let total_chars: usize = chunks.iter().filter_map(|c| c.delta_text.as_ref()).map(|t| t.len()).sum();
        let agent_id = AgentId("bench".to_string());

        // ---- Warmup ----
        let old_sink = OldPathSink::default();
        let new_sink = NewPathSink::default();
        let old_text = Mutex::new(String::new());
        let old_reasoning = Mutex::new(String::new());
        let ft = Mutex::new(0usize);
        let fr = Mutex::new(0usize);
        for _ in 0..5 {
            for chunk in &chunks {
                stream_assistant_chunk(
                    Some(&old_sink), &agent_id, &old_text, &old_reasoning, &ft, &fr, chunk.clone(), &[],
                );
            }
        }
        let new_text = Mutex::new(String::new());
        let new_reasoning = Mutex::new(String::new());
        let ft2 = Mutex::new(0usize);
        let fr2 = Mutex::new(0usize);
        for _ in 0..5 {
            for chunk in &chunks {
                stream_assistant_chunk(
                    Some(&new_sink), &agent_id, &new_text, &new_reasoning, &ft2, &fr2, chunk.clone(), &[],
                );
            }
        }

        // ---- Measure old path (full-text clone) ----
        let old_text = Mutex::new(String::new());
        let old_reasoning = Mutex::new(String::new());
        let ft3 = Mutex::new(0usize);
        let fr3 = Mutex::new(0usize);
        let old_start = Instant::now();
        for chunk in &chunks {
            stream_assistant_chunk(
                Some(&old_sink), &agent_id, &old_text, &old_reasoning, &ft3, &fr3, chunk.clone(), &[],
            );
        }
        let old_elapsed = old_start.elapsed();

        // ---- Measure new path (delta) ----
        let new_text = Mutex::new(String::new());
        let new_reasoning = Mutex::new(String::new());
        let ft4 = Mutex::new(0usize);
        let fr4 = Mutex::new(0usize);
        let new_start = Instant::now();
        for chunk in &chunks {
            stream_assistant_chunk(
                Some(&new_sink), &agent_id, &new_text, &new_reasoning, &ft4, &fr4, chunk.clone(), &[],
            );
        }
        let new_elapsed = new_start.elapsed();

        let old_us = old_elapsed.as_micros();
        let new_us = new_elapsed.as_micros();
        let ratio = if new_us > 0 { old_us as f64 / new_us as f64 } else { f64::INFINITY };

        eprintln!();
        eprintln!("=== stream_assistant_chunk microbenchmark ===");
        eprintln!("  Chunks: {num_chunks} × ~40 chars = ~{total_chars} chars total response");
        eprintln!("  Old path (full-text clone): {old_us:>8} µs  ({:.2} µs/chunk)", old_us as f64 / num_chunks as f64);
        eprintln!("  New path (delta):          {new_us:>8} µs  ({:.2} µs/chunk)", new_us as f64 / num_chunks as f64);
        eprintln!("  Speedup: {ratio:.1}× faster");
        eprintln!();

        // Assert that new path is at least as fast (not significantly slower).
        // Allow 20% tolerance for noise.
        assert!(
            new_us <= old_us + old_us / 5,
            "New path ({new_us}µs) should not be significantly slower than old path ({old_us}µs)"
        );
    }
}
