//! Tool request validation, batch execution, and result recording.
use super::{
    agent_id_or_anonymous, append_assistant_to_history, build_messages,
    extract_secrets_from_messages, filter_secrets_in_text, llm_call_with_recovery, now_ms,
    LoopContext,
};
use crate::input::LoopStopRule;
use crate::loop_state::LoopState;
use crate::suspend::SuspendedToolCall;
use agent_contracts::tool::{ToolCall, ToolFilter};
use agent_contracts::RuntimeView;
use agent_llm::{AssistantMessageExt, ChatMessageExt};
use agent_types::events::ToolResultEvent;
use agent_types::outcome::AgentError;
use agent_types::tool::{
    EffectProfile, FinalToolCall, RawToolCall, RawToolOutcome, ToolExecutionResult,
};
use agent_types::{AssistantMessage, ChatMessage, ToolUseBlock};
use serde_json::Value;
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use tool::{tool_filter_from_specs, ToolCallBuilderImpl};

pub(super) async fn run(ctx: &mut LoopContext<'_>) -> Result<Vec<SuspendedToolCall>, AgentError> {
    let Some(message) = &ctx.turn.assistant_message else {
        return Ok(Vec::new());
    };
    if !message.has_tool_calls()
        || !ctx.snapshot.feature_flags.tool_execution
        || ctx.input.runtime_view.is_none()
        || ctx.input.agent_id.is_none()
    {
        append_assistant_to_history(ctx);
        return Ok(Vec::new());
    }

    let calls = process_llm_tool_calls(ctx).await?;
    let filter = tool_filter_from_specs(
        &ctx.input.visible_tools,
        ctx.snapshot.tool_registry.as_ref(),
    );
    let runtime = ctx.input.runtime_view.clone().unwrap();
    let built: Vec<_> = calls
        .into_iter()
        .map(|call| {
            build_tool_call(
                RawToolCall {
                    call_id: call.call_id,
                    tool_name: call.tool_name,
                    input: call.input,
                },
                filter.as_ref(),
            )
        })
        .collect();
    let serialize = !built
        .iter()
        .filter_map(|call| call.as_ref().ok())
        .all(|call| {
            filter
                .get_spec_for_name(&call.final_call().tool_name)
                .is_some_and(|spec| is_parallel_safe(spec.effect_profile()))
        });
    let results = execute_tool_batch(built, runtime.as_ref(), serialize).await;
    Ok(record_tool_results(ctx, results).await)
}

fn normalized_tool_calls(ctx: &mut LoopContext<'_>) -> (Vec<ToolUseBlock>, Vec<ToolUseBlock>) {
    let Some(message) = ctx.turn.assistant_message.as_mut() else {
        return (Vec::new(), Vec::new());
    };
    synthesize_missing_call_ids(message, ctx.state.turn_count);
    repair_tool_names(message, &ctx.input.visible_tools);
    message
        .tool_calls
        .iter()
        .cloned()
        .partition(is_valid_tool_call)
}

async fn process_llm_tool_calls(
    ctx: &mut LoopContext<'_>,
) -> Result<Vec<ToolUseBlock>, AgentError> {
    let (mut valid_calls, mut invalid_calls) = normalized_tool_calls(ctx);
    // Normalization has already supplied missing IDs, so a single invalid call
    // can always receive a paired error result. Retry at most once.
    if valid_calls.is_empty() && invalid_calls.len() == 1 {
        let invalid = &invalid_calls[0];
        tracing::warn!(call_id = %invalid.call_id, "LLM returned an invalid tool call; retrying");
        ctx.state
            .messages
            .write()
            .push(build_invalid_tool_call_result(invalid));
        let retry = async {
            build_messages(ctx).await.map_err(|e| {
                AgentError::PromptBuild(format!("retry after invalid tool call: {e}"))
            })?;
            llm_call_with_recovery(ctx).await
        }
        .await;
        ctx.state.messages.write().pop();
        retry?;
        (valid_calls, invalid_calls) = normalized_tool_calls(ctx);
    }

    valid_calls.sort_by_key(|tc| tc.tool_name == "join_subagent");

    if let Some(msg) = ctx.turn.assistant_message.as_mut() {
        msg.tool_calls = valid_calls.clone();
    }
    append_assistant_to_history(ctx);

    for call in invalid_calls {
        tracing::warn!(call_id = %call.call_id, tool_name = %call.tool_name,
            "Discarding invalid tool call from LLM response");
        emit_tool_event(
            ctx,
            &call.call_id,
            &call.tool_name,
            &call.input,
            invalid_tool_call_message(&call),
            true,
        );
    }

    Ok(valid_calls)
}

// Build errors remain in the same ordered batch as executable calls.
type PreparedToolCall = Result<Box<dyn ToolCall>, ToolExecutionResult>;

pub(super) fn build_tool_call(raw: RawToolCall, filter: &dyn ToolFilter) -> PreparedToolCall {
    let fallback = FinalToolCall {
        call_id: raw.call_id.clone(),
        tool_name: raw.tool_name.clone(),
        input: raw.input.clone(),
        ..Default::default()
    };
    ToolCallBuilderImpl::build_with_filter_ref(raw, filter).map_err(|error| {
        build_framework_failed_tool_result(fallback, format!("tool call build failed: {error}"))
    })
}

pub(super) async fn execute_tool_call(
    call: PreparedToolCall,
    runtime: &dyn RuntimeView,
) -> ToolExecutionResult {
    let call = match call {
        Ok(call) => call,
        Err(result) => return result,
    };
    call.execute(runtime).await.unwrap_or_else(|error| {
        build_framework_failed_tool_result(call.final_call().clone(), error.to_string())
    })
}

pub(super) async fn execute_tool_batch(
    calls: Vec<PreparedToolCall>,
    runtime: &dyn RuntimeView,
    serialize: bool,
) -> Vec<ToolExecutionResult> {
    let executions = calls
        .into_iter()
        .map(|call| execute_tool_call(call, runtime));
    if serialize {
        let mut results = Vec::new();
        for execution in executions {
            results.push(execution.await);
        }
        results
    } else {
        futures_util::future::join_all(executions).await
    }
}

async fn record_tool_results(
    ctx: &mut LoopContext<'_>,
    results: Vec<ToolExecutionResult>,
) -> Vec<SuspendedToolCall> {
    // Record in call order; suspended calls were sorted last during preparation.
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

        let mut tool_result_message = build_tool_result_message(&result);
        ctx.estimator.update_message_cache(&mut tool_result_message);
        ctx.state.messages.write().push(tool_result_message);
        // Track repeated identical failing calls; any note is pushed after all
        // tool results so the assistant/tool-result protocol stays intact.
        if let Some(note) = update_tool_streak(ctx.state, &result) {
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
            let mut msg = ChatMessage::user(note);
            ctx.estimator.update_message_cache(&mut msg);
            ctx.state.messages.write().push(msg);
        }
    }

    suspended_calls
}

const REPEATED_FAILURE_THRESHOLD: u32 = 3;
const REPEATED_SUCCESS_THRESHOLD: u32 = 3;

fn update_tool_streak(state: &mut LoopState, result: &ToolExecutionResult) -> Option<String> {
    let failed = is_failure_result(result);
    let signature = tool_call_signature(result);
    let (last, count, threshold) = if failed {
        state.last_success_sig = None;
        state.repeated_success_count = 0;
        (
            &mut state.last_failure_sig,
            &mut state.repeated_failure_count,
            REPEATED_FAILURE_THRESHOLD,
        )
    } else {
        state.last_failure_sig = None;
        state.repeated_failure_count = 0;
        (
            &mut state.last_success_sig,
            &mut state.repeated_success_count,
            REPEATED_SUCCESS_THRESHOLD,
        )
    };
    *count = if *last == Some(signature) {
        *count + 1
    } else {
        1
    };
    *last = Some(signature);
    if *count < threshold {
        return None;
    }
    let repetitions = *count;
    *last = None;
    *count = 0;
    let tool = result.tool_name();
    Some(if failed {
        format!("The `{tool}` call has now failed {repetitions} times in a row with identical arguments. \
                 Stop retrying it unchanged — change approach: fix the arguments, read the relevant \
                 file or state to understand why it fails, or use a different tool to reach the goal.")
    } else {
        format!("The `{tool}` call has now run {repetitions} times in a row with identical arguments and the \
                 same result — that output is already in your context above. Stop repeating it: use \
                 what you have, or take a different action toward the goal.")
    })
}

pub(super) fn is_parallel_safe(profile: &EffectProfile) -> bool {
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
    if let ToolExecutionResult::Completed {
        raw_outcome: RawToolOutcome::Success { output },
        ..
    } = result
    {
        output.hash(&mut hasher);
    }
    hasher.finish()
}

/// Fill empty `call_id`s with a stable, turn-scoped id (`call_<turn>_<idx>`) so a
/// provider that omits `tool_call.id` does not get the call rejected as invalid; a
/// non-empty id is left untouched.
pub(super) fn synthesize_missing_call_ids(msg: &mut AssistantMessage, turn: u32) {
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

pub(super) fn is_valid_tool_call(tc: &ToolUseBlock) -> bool {
    is_valid_tool_call_id(&tc.call_id) && is_valid_tool_name(&tc.tool_name)
}

fn is_valid_tool_call_id(call_id: &str) -> bool {
    !call_id.trim().is_empty()
}

fn is_valid_tool_name(name: &str) -> bool {
    !name.trim().is_empty()
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
    final_call: FinalToolCall,
    message: String,
) -> ToolExecutionResult {
    ToolExecutionResult::Failed {
        final_call,
        pre_hook_results: Vec::new(),
        error_hook_results: Vec::new(),
        execution_error: agent_types::tool::ToolExecutionError::ExecutionFailed { message },
    }
}

/// Borrow potentially large successful output; allocate only formatted errors.
fn tool_output(result: &ToolExecutionResult) -> Cow<'_, str> {
    match result {
        ToolExecutionResult::Completed { raw_outcome, .. } => match raw_outcome {
            RawToolOutcome::Success { output } => Cow::Borrowed(output),
            RawToolOutcome::Error { message } => Cow::Borrowed(message),
        },
        ToolExecutionResult::Failed {
            execution_error, ..
        } => Cow::Owned(execution_error.to_string()),
        ToolExecutionResult::Denied { error, .. } => Cow::Owned(format!(
            "denied: {}",
            error.as_ref().map(ToString::to_string).unwrap_or_default()
        )),
        ToolExecutionResult::Suspended { suspend_token, .. } => {
            Cow::Owned(format!("suspended:{suspend_token}"))
        }
    }
}

fn emit_tool_result_event(ctx: &LoopContext<'_>, result: &ToolExecutionResult) {
    if ctx.input.event_sink.is_none() || matches!(result, ToolExecutionResult::Suspended { .. }) {
        return;
    }
    let output = tool_output(result);
    let preview = if result.tool_name() == "ask_user_question" && !is_failure_result(result) {
        filter_ask_user_question_output(&output)
    } else {
        output.into_owned()
    };
    let call = result.final_call();
    emit_tool_event(
        ctx,
        &call.call_id,
        &call.tool_name,
        &call.input,
        preview,
        is_failure_result(result),
    );
}

fn emit_tool_event(
    ctx: &LoopContext<'_>,
    call_id: &str,
    tool_name: &str,
    input: &Value,
    output_preview: String,
    is_error: bool,
) {
    let Some(sink) = &ctx.input.event_sink else {
        return;
    };
    // Other tools need neither history scanning nor an input clone.
    let args_preview = if tool_name == "bash" {
        let secrets = ctx.state.with_messages(extract_secrets_from_messages);
        bash_args_preview(input, &secrets)
    } else {
        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
    };
    sink.on_tool_result(
        agent_id_or_anonymous(ctx.input.agent_id.as_ref()),
        &ToolResultEvent {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            output_preview,
            is_error,
            args_preview,
        },
    );
}

fn bash_args_preview(input: &Value, secrets: &[String]) -> String {
    let mut input = input.clone();
    if let Some(Value::String(command)) = input.get_mut("command") {
        *command = filter_secrets_in_text(command, secrets);
    }
    serde_json::to_string_pretty(&input).unwrap_or_else(|_| input.to_string())
}

/// The UI uses display values for secret answers and limits the preview length.
pub(super) fn filter_ask_user_question_output(output: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(output) else {
        return output.chars().take(200).collect();
    };
    if let Some(answers) = value.get_mut("answers").and_then(Value::as_array_mut) {
        for answer in answers {
            let Some(answer) = answer.as_object_mut() else {
                continue;
            };
            if answer.get("kind").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let Some(display) = answer
                .get("display_value")
                .filter(|v| !v.is_null())
                .cloned()
            else {
                continue;
            };
            answer.insert("value".into(), display);
            answer.remove("display_value");
        }
    }
    value.to_string().chars().take(200).collect()
}

pub(super) fn should_stop_after_tool_result(
    ctx: &LoopContext<'_>,
    result: &ToolExecutionResult,
) -> bool {
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

pub fn build_tool_result_message(result: &ToolExecutionResult) -> ChatMessage {
    build_tool_result_message_with_appendix(result, "")
}

pub(super) fn build_tool_result_message_with_appendix(
    result: &ToolExecutionResult,
    appendix: &str,
) -> ChatMessage {
    let call = result.final_call();
    let mut output = tool_output(result);
    if !appendix.is_empty()
        && matches!(
            result,
            ToolExecutionResult::Completed {
                raw_outcome: RawToolOutcome::Success { .. },
                ..
            }
        )
    {
        output.to_mut().push_str(appendix);
    }
    ChatMessage::tool_result(
        call.call_id.clone(),
        call.tool_name.clone(),
        truncate_tool_output(&call.tool_name, &call.call_id, &output),
        is_failure_result(result),
        now_ms(),
    )
}

/// Primary truncation threshold on the *byte count* of tool output.
pub(super) const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
/// Secondary truncation threshold on the *line count* of tool output.
/// Whichever triggers first drives the in-context preview. Mirrors
/// opencode's `MAX_LINES = 2000` and the grep tool's `ABSOLUTE_HARD_CAP`.
pub(super) const MAX_TOOL_OUTPUT_LINES: usize = 2_000;
const TRUNCATED_TOOL_OUTPUT_DIR: &str = "truncated_tool_output";
const TRUNCATED_RETENTION_DAYS: u64 = 7;

/// Lazy-cleanup day-counter — guards `maybe_cleanup_truncated_dir` to run
/// at most once per day so truncation calls stay cheap.
static LAST_CLEANUP_DAY: AtomicU64 = AtomicU64::new(0);

/// Returns the largest byte index `<= index` that falls on a UTF-8 character
/// boundary.  Equivalent to `str::floor_char_boundary` (stable since Rust
/// 1.80) but written inline so it compiles on any Rust version the CI may
/// pin.
pub(super) fn char_boundary_before(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let bytes = s.as_bytes();
    let mut i = index;
    while i > 0 && bytes[i] & 0xC0 == 0x80 {
        i -= 1;
    }
    i
}

/// Truncates tool output to fit within [`MAX_TOOL_OUTPUT_BYTES`] bytes
/// AND [`MAX_TOOL_OUTPUT_LINES`] lines (whichever triggers first), then
/// saves the full content to `~/.xiaoo/truncated_tool_output/`.
pub(super) fn truncate_tool_output(tool_name: &str, call_id: &str, output: &str) -> String {
    let total_bytes = output.len();

    // Fast path: fits both thresholds.
    if total_bytes <= MAX_TOOL_OUTPUT_BYTES {
        let n = output.lines().count();
        if n <= MAX_TOOL_OUTPUT_LINES {
            return output.to_string();
        }
    }

    // Slow path: single pass that counts total lines AND builds the preview.
    // Once a limit fires, stop pushing but keep counting for the hint.
    let mut total_lines = 0usize;
    let mut kept_bytes = 0usize;
    let mut kept_lines = 0usize;
    let mut hit_byte_limit = false;
    let mut preview_done = false;
    let mut preview = String::new();
    for line in output.lines() {
        total_lines += 1;
        if preview_done {
            continue;
        }
        let line_size = line.len() + if preview.is_empty() { 0 } else { 1 };
        if kept_lines + 1 > MAX_TOOL_OUTPUT_LINES {
            preview_done = true;
            continue;
        }
        if kept_bytes + line_size > MAX_TOOL_OUTPUT_BYTES {
            hit_byte_limit = true;
            preview_done = true;
            continue;
        }
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(line);
        kept_bytes += line_size;
        kept_lines += 1;
    }

    // Byte-level fallback for single long lines.
    if preview.is_empty() && total_bytes > 0 {
        let boundary = char_boundary_before(output, MAX_TOOL_OUTPUT_BYTES);
        preview.push_str(&output[..boundary]);
        kept_bytes = boundary;
        kept_lines = 0;
        hit_byte_limit = true;
    }

    let omitted_bytes = total_bytes.saturating_sub(kept_bytes);
    let omitted_lines = total_lines.saturating_sub(kept_lines);

    let saved_path = std::env::var("HOME").ok().and_then(|home| {
        let dir = std::path::PathBuf::from(home)
            .join(".xiaoo")
            .join(TRUNCATED_TOOL_OUTPUT_DIR);
        std::fs::create_dir_all(&dir).ok()?;
        let filename = format!("{}.{}.{}.txt", tool_name, now_ms(), call_id);
        let path = dir.join(&filename);
        std::fs::write(&path, output).ok().map(|_| {
            maybe_cleanup_truncated_dir(&dir);
            path
        })
    });

    let trigger = if hit_byte_limit {
        "byte limit"
    } else {
        "line limit"
    };

    let location = saved_path
        .map(|path| format!("Full output saved to: {}\n", path.display()))
        .unwrap_or_default();
    format!(
        "{preview}\n[Tool output truncated ({trigger}): omitted {omitted_bytes} bytes ({omitted_lines} lines), \
         showing first {kept_bytes} bytes ({kept_lines} lines)]\n\
         {location}Use `file_read` with offset/limit or `grep` to search the full content."
    )
}

/// Lazy cleanup of truncated tool output files older than
/// [`TRUNCATED_RETENTION_DAYS`]. Runs at most once per day to avoid
/// unnecessary filesystem scans on every truncated tool call.
fn maybe_cleanup_truncated_dir(dir: &std::path::Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap_or(0);
    let last = LAST_CLEANUP_DAY.load(Ordering::Relaxed);
    if last >= now {
        return;
    }
    LAST_CLEANUP_DAY.store(now, Ordering::Relaxed);

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let retention = std::time::Duration::from_secs(TRUNCATED_RETENTION_DAYS * 86400);
    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Ok(age) = mtime.elapsed() {
            if age > retention {
                std::fs::remove_file(entry.path()).ok();
            }
        }
    }
}
