use std::borrow::Cow;

use agent_contracts::runtime::RuntimeView;
use agent_contracts::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_types::llm::{LlmError, LlmRequest, LlmResponse, ResponseFormat};
use serde_json::{json, Value};

#[derive(Default)]
pub(super) struct StreamTraceStats {
    pub(super) chunk_count: usize,
    pub(super) text_bytes: usize,
    pub(super) saw_tool_call_delta: bool,
}

pub(super) async fn begin_trace_span(
    runtime_view: Option<&dyn RuntimeView>,
    provider_model_name: &str,
    request: &LlmRequest,
    is_streaming: bool,
) -> Option<TraceSpanHandle> {
    let runtime_view = runtime_view?;
    Some(
        runtime_view
            .trace_recorder()
            .begin_span(
                TraceSpanKind::LlmCall,
                Cow::Borrowed("llm_call"),
                json!({
                    "agent_id": runtime_view.agent_context().metadata().agent_id,
                    "runtime_model": runtime_view.agent_context().metadata().model,
                    "session_id": runtime_view.agent_context().metadata().session_id,
                    "provider_model": provider_model_name,
                    "request_message_count": request.messages.len(),
                    "tool_count": request.tools.len(),
                    "tool_choice": request.tool_choice,
                    "max_tokens": request.max_tokens,
                    "temperature": request.temperature,
                    "has_max_tokens": request.max_tokens.is_some(),
                    "has_temperature": request.temperature.is_some(),
                    "response_format": response_format_name(&request.response_format),
                    "is_streaming": is_streaming,
                    "phase": "started",
                }),
            )
            .await,
    )
}

pub(super) async fn update_trace_span(
    runtime_view: Option<&dyn RuntimeView>,
    span: Option<&TraceSpanHandle>,
    fields: Value,
) {
    let (Some(runtime_view), Some(span)) = (runtime_view, span) else {
        return;
    };

    runtime_view
        .trace_recorder()
        .update_span(span, fields)
        .await;
}

pub(super) async fn end_trace_span(
    runtime_view: Option<&dyn RuntimeView>,
    span: &mut Option<TraceSpanHandle>,
    outcome: TraceOutcome,
    fields: Value,
) {
    let (Some(runtime_view), Some(span)) = (runtime_view, span.take()) else {
        return;
    };

    runtime_view
        .trace_recorder()
        .end_span(span, outcome, fields)
        .await;
}

pub(super) fn trace_outcome_for_error(error: &LlmError) -> TraceOutcome {
    match error {
        LlmError::Cancelled => TraceOutcome::Cancelled,
        _ => TraceOutcome::Error,
    }
}

pub(super) fn llm_error_kind(error: &LlmError) -> &'static str {
    match error {
        LlmError::RequestFailed { .. } => "request_failed",
        LlmError::HttpError(_) => "http_error",
        LlmError::ApiError(_) => "api_error",
        LlmError::ParseError(_) => "parse_error",
        LlmError::RateLimited { .. } => "rate_limited",
        LlmError::AuthError { .. } => "auth_error",
        LlmError::ModelNotFound { .. } => "model_not_found",
        LlmError::ProviderNotFound(_) => "provider_not_found",
        LlmError::ConfigError(_) => "config_error",
        LlmError::ContextLengthExceeded { .. } => "context_length_exceeded",
        LlmError::StreamError { .. } => "stream_error",
        LlmError::IoError(_) => "io_error",
        LlmError::Timeout => "timeout",
        LlmError::Cancelled => "cancelled",
    }
}

pub(super) fn response_trace_fields(response: &LlmResponse) -> Value {
    json!({
        "final_response": response_trace_payload(response),
        "stop_reason": format!("{:?}", response.message.stop_reason),
        "prompt_tokens": response.message.usage.prompt_tokens,
        "completion_tokens": response.message.usage.completion_tokens,
        "total_tokens": response.message.usage.total_tokens,
        "response_has_text": response.message.text.is_some(),
        "response_text_len": response.message.text.as_ref().map(|text| text.len()),
        "tool_call_count": response.message.tool_calls.len(),
    })
}

pub(super) fn effective_request_trace_fields(request: &LlmRequest) -> Value {
    json!({
        "effective_request": request_trace_payload(request),
        "request_message_count": request.messages.len(),
        "tool_count": request.tools.len(),
        "tool_choice": request.tool_choice,
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "has_max_tokens": request.max_tokens.is_some(),
        "has_temperature": request.temperature.is_some(),
        "response_format": response_format_name(&request.response_format),
    })
}

pub(super) fn error_trace_fields(error: &LlmError) -> Value {
    json!({
        "error_kind": llm_error_kind(error),
        "error_message": error.to_string(),
    })
}

pub(super) fn stream_trace_fields(stats: &StreamTraceStats) -> Value {
    json!({
        "chunk_count": stats.chunk_count,
        "stream_text_bytes": stats.text_bytes,
        "saw_tool_call_delta": stats.saw_tool_call_delta,
    })
}

pub(super) fn merge_trace_fields(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base_map), Value::Object(extra_map)) => {
            for (key, value) in extra_map {
                base_map.insert(key, value);
            }
            Value::Object(base_map)
        }
        (base, _) => base,
    }
}

fn response_format_name(response_format: &ResponseFormat) -> &'static str {
    match response_format {
        ResponseFormat::Text => "text",
        ResponseFormat::JsonObject => "json_object",
        ResponseFormat::JsonSchema { .. } => "json_schema",
    }
}

fn request_trace_payload(request: &LlmRequest) -> Value {
    serde_json::to_value(request).expect("LlmRequest should serialize for trace payload")
}

fn response_trace_payload(response: &LlmResponse) -> Value {
    serde_json::to_value(&response.message)
        .expect("AssistantMessage should serialize for trace payload")
}
