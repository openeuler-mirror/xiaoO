use std::sync::Mutex;

use agent_contracts::context::prompt::input::PromptBuildInput;
use agent_contracts::events::LoopEventSink;
use agent_contracts::tool::ToolCallBuilder;
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_llm::{AssistantMessageExt, ChatMessageExt};
use agent_types::compression::CompressedView;
use agent_types::context::prompt::result::PromptBuildResult;
use agent_types::events::ToolResultEvent;
use agent_types::outcome::{AgentError, AgentOutcome};
use agent_types::tool::{RawToolCall, RawToolOutcome, ToolExecutionResult};
use agent_types::{
    AssistantMessage, ChatMessage, ContentBlock, MessageRole, StreamChunk, ToolUseBlock,
};
use serde_json::json;
use tool::ToolCallBuilderImpl;

use crate::input::AgentLoopInput;
use crate::loop_state::LoopState;
use crate::runtime::AgentRuntime;
use crate::snapshot::RuntimeSnapshot;
use crate::suspend::{LoopRunResult, SuspendedToolCall};

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

    // Detect `/skill-name` prefix and expand skill prompt inline.
    if input.append_user_message {
        if let Some(expanded) = try_expand_skill_prefix(&input.user_message, &*snapshot.skill_registry) {
            input.user_message = expanded;
        }
        state.messages.push(ChatMessage::text(
            MessageRole::User,
            &input.user_message,
            now_ms(),
        ));
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
            let default_agent_id = agent_types::common::ids::AgentId(String::from("anonymous"));
            let agent_id = ctx.input.agent_id.as_ref().unwrap_or(&default_agent_id);
            sink.on_turn_start(agent_id, ctx.turn.turn_number);
        }
        if let Err(error) = compress(&mut ctx).await {
            end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "compression_error"})).await;
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
            end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "prompt_build_error"})).await;
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Error,
                Some(error.to_string()),
                "prompt_build_error",
            )
            .await;
            return Err(error);
        }
        if let Err(error) = llm_call(&mut ctx).await {
            end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "llm_call_error"})).await;
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
        let suspended_call = match tool_exec(&mut ctx).await {
            Ok(suspended_call) => suspended_call,
            Err(error) => {
                end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "tool_exec_error"})).await;
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
        if let Some(suspended_call) = suspended_call {
            end_turn_span(&mut ctx, TraceOutcome::Ok, json!({"stop_reason": "suspended"})).await;
            emit_loop_end(&ctx, "suspended");
            finalize_trace_for_ctx(
                &ctx,
                TraceOutcome::Ok,
                Some("suspended".to_string()),
                "suspended",
            )
            .await;
            return Ok(LoopRunResult::Suspended(suspended_call));
        }
        decide(&mut ctx);

        match ctx.turn.decision {
            Some(LoopDecision::Continue) => {
                end_turn_span(&mut ctx, TraceOutcome::Ok, json!({"stop_reason": "continue"})).await;
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
                end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "max_turns"})).await;
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
                end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "budget_exhausted"})).await;
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
                end_turn_span(&mut ctx, TraceOutcome::Cancelled, json!({"stop_reason": "cancelled"})).await;
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
                end_turn_span(&mut ctx, TraceOutcome::Error, json!({"stop_reason": "missing_decision"})).await;
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
    end_turn_span(&mut ctx, TraceOutcome::Ok, json!({"stop_reason": "complete"})).await;

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
        messages: ctx.state.messages.clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
    }))
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
    let agent_id = ctx
        .input
        .agent_id
        .as_ref()
        .map(|id| id.0.as_str())
        .unwrap_or("anonymous")
        .to_string();
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
    let (prompt_tokens, completion_tokens, total_tokens, has_tool_calls) =
        match ctx.turn.assistant_message.as_ref() {
            Some(msg) => (
                msg.usage.prompt_tokens,
                msg.usage.completion_tokens,
                msg.usage.total_tokens,
                msg.has_tool_calls(),
            ),
            None => (0, 0, 0, false),
        };
    runtime_view
        .trace_recorder()
        .update_span(
            span,
            json!({
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": total_tokens,
                "has_tool_calls": has_tool_calls,
            }),
        )
        .await;
}

async fn end_turn_span(ctx: &mut LoopContext<'_>, outcome: TraceOutcome, fields: serde_json::Value) {
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

async fn compress(ctx: &mut LoopContext<'_>) -> Result<(), AgentError> {
    let agent_id_str = ctx
        .input
        .agent_id
        .as_ref()
        .map(|id| id.0.clone())
        .unwrap_or_default();

    // begin span — 记录开始时的基础元数据
    let compression_span = if let Some(rv) = ctx.input.runtime_view.clone() {
        Some(
            rv.trace_recorder()
                .begin_span(
                    TraceSpanKind::Compression,
                    std::borrow::Cow::Borrowed("compression"),
                    json!({
                        "turn_number": ctx.turn.turn_number,
                        "agent_id": agent_id_str,
                        "message_count": ctx.state.messages.len(),
                    }),
                )
                .await,
        )
    } else {
        None
    };

    let analysis = ctx
        .snapshot
        .compression_pipeline
        .analyze(&ctx.state.messages, &*ctx.snapshot.token_budget_policy);

    tracing::debug!(
        estimated = analysis.estimated_tokens,
        available = analysis.available_tokens,
        ratio = format!("{:.1}%", analysis.usage_ratio * 100.0),
        severity = ?analysis.severity,
        msg_count = ctx.state.messages.len(),
        "compression analysis"
    );

    // update span — 记录分析结果
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
                }),
            )
            .await;
    }

    if !analysis.needs_compression() {
        // end span — 无需压缩，正常结束
        if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span) {
            rv.trace_recorder()
                .end_span(
                    span,
                    TraceOutcome::Ok,
                    json!({ "skipped": true }),
                )
                .await;
        }
        return Ok(());
    }

    let msg_count_before = ctx.state.messages.len();

    let view = ctx
        .snapshot
        .compression_pipeline
        .compress(
            &ctx.state.messages,
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
                "context compression triggered"
            );

            // end span — 压缩成功，记录输出信息
            if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span) {
                rv.trace_recorder()
                    .end_span(
                        span,
                        TraceOutcome::Ok,
                        json!({
                            "skipped": false,
                            "messages_before": msg_count_before,
                            "messages_after": view.messages.len(),
                            "removed_count": view.removed_count,
                            "has_summary": view.summary.is_some(),
                            "estimated_tokens_after": view.estimated_tokens,
                        }),
                    )
                    .await;
            }

            ctx.state.messages = view.messages.clone();
            ctx.state.compression_meta = view.updated_meta.clone();
            ctx.turn.compression_output = Some(view);

            Ok(())
        }
        Err(e) => {
            // end span — 压缩失败，记录错误信息
            if let (Some(rv), Some(span)) = (ctx.input.runtime_view.clone(), compression_span) {
                rv.trace_recorder()
                    .end_span(
                        span,
                        TraceOutcome::Error,
                        json!({ "error": e.to_string() }),
                    )
                    .await;
            }
            Err(e)
        }
    }
}

fn microcompact(ctx: &mut LoopContext<'_>) {
    let result = ctx
        .snapshot
        .compression_pipeline
        .microcompact(&ctx.state.messages, now_ms());

    if result.applied {
        tracing::info!(
            removed = result.removed_count,
            removed_call_ids = ?result.removed_call_ids,
            token_delta = result.token_delta,
            "microcompact applied"
        );
        ctx.state.messages = result.messages;
    }
}

async fn build_messages(ctx: &mut LoopContext<'_>) -> Result<(), AgentError> {
    let skill_summaries = ctx.snapshot.skill_registry.list_skills();

    let agent_id_str = ctx
        .input
        .agent_id
        .as_ref()
        .map(|id| id.0.clone())
        .unwrap_or_default();

    // begin span — 记录开始时的基础元数据
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

    let input = PromptBuildInput {
        system_prompt: ctx.snapshot.system_prompt.to_string(),
        messages: ctx.state.messages.clone(),
        visible_tools: ctx.input.visible_tools.clone(),
        skill_summaries,
        memory_snippets: Vec::new(),
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

    // update span — 记录构建完成的 input 维度信息
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
        Ok(result) => {
            // end span — 成功，记录估算 token 数等输出信息
            if let (Some(rv), Some(span)) =
                (ctx.input.runtime_view.clone(), prompt_build_span)
            {
                rv.trace_recorder()
                    .end_span(
                        span,
                        TraceOutcome::Ok,
                        json!({
                            "estimated_input_tokens": result.estimated_input_tokens,
                            "request_message_count": result.request.messages.len(),
                        }),
                    )
                    .await;
            }
            ctx.turn.build_messages_output = Some(result);
            Ok(())
        }
        Err(e) => {
            // end span — 失败，记录错误信息
            if let (Some(rv), Some(span)) =
                (ctx.input.runtime_view.clone(), prompt_build_span)
            {
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

async fn llm_call(ctx: &mut LoopContext<'_>) -> Result<(), AgentError> {
    if ctx.state.cancel.is_cancelled() {
        return Ok(());
    }

    let build_result = ctx
        .turn
        .build_messages_output
        .as_ref()
        .expect("build_messages must run before llm_call");

    let event_sink = ctx.input.event_sink.clone();
    let streamed_text = Mutex::new(String::new());
    let response = ctx
        .snapshot
        .llm_provider
        .complete_stream(&build_result.request, &|chunk| {
            let default_agent_id = agent_types::common::ids::AgentId(String::from("anonymous"));
            let agent_id = ctx
                .input
                .agent_id
                .as_ref()
                .unwrap_or(&default_agent_id)
                .clone();
            stream_assistant_text(event_sink.as_deref(), &agent_id, &streamed_text, chunk);
        })
        .await
        .map_err(|e| AgentError::LlmProvider(e.to_string()))?;

    ctx.state.token_usage.prompt_tokens += response.message.usage.prompt_tokens;
    ctx.state.token_usage.completion_tokens += response.message.usage.completion_tokens;
    ctx.state.token_usage.total_tokens += response.message.usage.total_tokens;

    let streamed_text = streamed_text
        .into_inner()
        .expect("assistant stream text mutex should not be poisoned");
    if let Some(ref sink) = event_sink {
        if let Some(ref text) = response.message.text {
            if streamed_text != *text {
                let default_agent_id = agent_types::common::ids::AgentId(String::from("anonymous"));
                let agent_id = ctx.input.agent_id.as_ref().unwrap_or(&default_agent_id);
                sink.on_assistant_message(agent_id, text);
            }
        }
    }

    ctx.turn.assistant_message = Some(response.message);
    Ok(())
}

fn stream_assistant_text(
    sink: Option<&dyn LoopEventSink>,
    agent_id: &agent_types::common::ids::AgentId,
    streamed_text: &Mutex<String>,
    chunk: StreamChunk,
) {
    let Some(delta_text) = chunk.delta_text else {
        return;
    };

    let snapshot = {
        let mut full_text = streamed_text
            .lock()
            .expect("assistant stream text mutex should not be poisoned");
        full_text.push_str(&delta_text);
        full_text.clone()
    };

    if let Some(sink) = sink {
        sink.on_assistant_message(agent_id, &snapshot);
    }
}

async fn tool_exec(ctx: &mut LoopContext<'_>) -> Result<Option<SuspendedToolCall>, AgentError> {
    let has_tool_calls = ctx
        .turn
        .assistant_message
        .as_ref()
        .map_or(false, |m| m.has_tool_calls());

    if ctx.turn.assistant_message.is_none() {
        return Ok(None);
    }

    if !has_tool_calls || !ctx.snapshot.feature_flags.tool_execution {
        append_assistant_to_history(ctx);
        return Ok(None);
    }

    if ctx.input.runtime_view.is_none() {
        append_assistant_to_history(ctx);
        return Ok(None);
    }

    let tool_calls: Vec<ToolUseBlock> = ctx
        .turn
        .assistant_message
        .as_ref()
        .unwrap()
        .tool_calls
        .clone();

    append_assistant_to_history(ctx);

    let agent_id = match ctx.input.agent_id {
        Some(ref id) => id,
        None => return Ok(None),
    };

    let runtime_view = ctx.input.runtime_view.as_ref().unwrap();

    for tc in &tool_calls {
        let raw_tool_call = RawToolCall {
            call_id: tc.call_id.clone(),
            tool_name: tc.tool_name.clone(),
            input: tc.input.clone(),
        };

        let per_call_filter = ctx.snapshot.tool_registry.filter_for(agent_id);

        let tool_call = ToolCallBuilderImpl::new()
            .with_raw_llm_tool_call(raw_tool_call)
            .with_tool_filter(per_call_filter)
            .build()
            .map_err(|e| AgentError::ToolExecution(e.to_string()))?;

        let result = tool_call
            .execute(&**runtime_view)
            .await
            .map_err(|e| AgentError::ToolExecution(e.to_string()))?;

        if let Some(ref sink) = ctx.input.event_sink {
            let mut should_emit = true;
            let (output_preview, is_error) = match &result {
                ToolExecutionResult::Completed { raw_outcome, .. } => {
                    let preview = match raw_outcome {
                        RawToolOutcome::Success { output } => output.chars().take(200).collect(),
                        RawToolOutcome::Error { message } => message.chars().take(200).collect(),
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
                let default_agent_id = agent_types::common::ids::AgentId(String::from("anonymous"));
                let agent_id = ctx.input.agent_id.as_ref().unwrap_or(&default_agent_id);
                sink.on_tool_result(
                    agent_id,
                    &ToolResultEvent {
                        call_id: result.call_id().to_string(),
                        tool_name: result.tool_name().to_string(),
                        output_preview,
                        is_error,
                    },
                );
            }
        }

        if let Some(suspended_call) = SuspendedToolCall::from_tool_result(&result) {
            ctx.turn.tool_results.push(result);
            return Ok(Some(suspended_call));
        }

        let tool_result_message = build_tool_result_message(&result);
        ctx.state.messages.push(tool_result_message);
        ctx.turn.tool_results.push(result);
    }

    Ok(None)
}

fn decide(ctx: &mut LoopContext<'_>) {
    if ctx.state.cancel.is_cancelled() {
        ctx.turn.decision = Some(LoopDecision::ReturnCancelled);
        return;
    }

    if ctx.state.turn_count + 1 >= ctx.snapshot.max_turns {
        ctx.turn.decision = Some(LoopDecision::ReturnMaxTurns);
        return;
    }

    // NOTE: no cumulative token budget check here.
    // Context window pressure is handled by the compression pipeline
    // (compress/microcompact) at the start of each turn.

    if let Some(ref msg) = ctx.turn.assistant_message {
        let can_execute_tool_calls = ctx.snapshot.feature_flags.tool_execution
            && !ctx.input.visible_tools.is_empty()
            && ctx.input.runtime_view.is_some();

        if msg.has_tool_calls() && can_execute_tool_calls {
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

    ctx.state.messages.push(ChatMessage {
        role: MessageRole::Assistant,
        blocks,
        message_id: None,
        timestamp_ms: now_ms(),
        api_usage_tokens: Some(msg.usage.total_tokens),
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
    }
}

fn emit_loop_end(ctx: &LoopContext<'_>, stop_reason: &str) {
    if let Some(ref sink) = ctx.input.event_sink {
        let default_agent_id = agent_types::common::ids::AgentId(String::from("anonymous"));
        let agent_id = ctx.input.agent_id.as_ref().unwrap_or(&default_agent_id);
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
        expanded.push_str(&format!(
            "[Skill directory: {}]\n\n",
            location.display()
        ));
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
        messages: ctx.state.messages.clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
    }
}

fn build_outcome_budget(ctx: &LoopContext<'_>) -> AgentOutcome {
    AgentOutcome::BudgetExhausted {
        partial_reply: ctx
            .turn
            .assistant_message
            .as_ref()
            .and_then(|m| m.text.clone()),
        messages: ctx.state.messages.clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
    }
}

fn build_outcome_cancelled(ctx: &LoopContext<'_>) -> AgentOutcome {
    AgentOutcome::Cancelled {
        partial_reply: ctx
            .turn
            .assistant_message
            .as_ref()
            .and_then(|m| m.text.clone()),
        messages: ctx.state.messages.clone(),
        turn_count: ctx.state.turn_count,
        token_usage: ctx.state.token_usage.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use agent_contracts::context::budget::TokenBudgetPolicy;
    use agent_contracts::{
        CompressionPipeline, LlmProvider, PromptBuilder, ProviderCapabilities, SkillRegistry,
    };
    use agent_llm::LlmRequestExt;
    use agent_types::common::ids::AgentId;
    use agent_types::context::budget::BudgetError;
    use agent_types::context::prompt::{PromptBuildError, PromptBuildResult};
    use agent_types::context::{FeatureFlags, TokenBudgetConfig};
    use agent_types::events::LoopEndSummary;
    use agent_types::{
        AssistantMessage, LlmError, LlmRequest, LlmResponse, StopReason, StreamChunk, Usage,
    };
    use async_trait::async_trait;
    use llm_client::LlmProviderWrapper;
    use tool::EmptyToolRegistry;

    use crate::runtime_support::EmptySkillRegistry;

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
                delta_tool_call: None,
            });
            on_chunk(StreamChunk {
                delta_text: Some(" world".to_string()),
                delta_tool_call: None,
            });

            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some("Hello world".to_string()),
                    tool_calls: Vec::new(),
                    usage: Usage {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                        total_tokens: 5,
                    },
                    stop_reason: StopReason::EndTurn,
                },
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
        let prompt_builder: Arc<dyn PromptBuilder> = Arc::new(FixedPromptBuilder);
        let compression_pipeline: Arc<dyn CompressionPipeline> =
            Arc::new(compact::PassthroughCompressionPipeline::new());
        let tool_registry = Arc::new(EmptyToolRegistry::new());
        let skill_registry: Arc<dyn SkillRegistry> = Arc::new(EmptySkillRegistry::new());
        let budget_config = TokenBudgetConfig {
            total_budget: 4096,
            reserved_for_output: 512,
            reserved_for_system: 256,
            hard_limit_ratio: 1.0,
        };
        let budget_policy: Arc<dyn TokenBudgetPolicy> =
            Arc::new(FixedBudgetPolicy::new(budget_config.clone()));
        let runtime = AgentRuntime::builder()
            .llm_provider(provider)
            .compression_pipeline(compression_pipeline)
            .prompt_builder(prompt_builder)
            .system_prompt("You are a coding agent.")
            .tool_registry(tool_registry)
            .skill_registry(skill_registry)
            .feature_flags(FeatureFlags::default())
            .max_turns(4)
            .token_budget_config(budget_config)
            .token_budget_policy(budget_policy)
            .build()
            .expect("test runtime should build");
        let sink = Arc::new(RecordingLoopEventSink::default());
        let input = AgentLoopInput::new("hello")
            .with_agent_id(AgentId("test-agent".to_string()))
            .with_event_sink(sink.clone());
        let mut loop_state = LoopState::new(uuid::Uuid::new_v4());

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
                .last()
                .and_then(ChatMessage::text_content),
            Some("Hello world")
        );
    }
}
