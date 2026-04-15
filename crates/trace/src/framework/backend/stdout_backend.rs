use super::backend_trait::TraceBackend;
use agent_contracts::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use async_trait::async_trait;
use serde_json::json;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use tokio::sync::Mutex;

pub(crate) struct StdoutBackend {
    spans: Mutex<HashMap<String, StdoutSpanState>>,
}

struct StdoutSpanState {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    start_time: u64,
    last_updated_at: u64,
    end_time: Option<u64>,
    kind: TraceSpanKind,
    name: String,
    fields: Value,
    outcome: Option<TraceOutcome>,
}

impl StdoutBackend {
    pub(crate) fn new() -> Self {
        Self {
            spans: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl TraceBackend for StdoutBackend {
    async fn begin_span(
        &self,
        occurred_at_ms: u64,
        span: &TraceSpanHandle,
        kind: TraceSpanKind,
        name: Cow<'static, str>,
        fields: Value,
    ) {
        let state = StdoutSpanState {
            trace_id: span.trace_id().to_string(),
            span_id: span.span_id().to_string(),
            parent_span_id: span.parent_span_id().map(ToString::to_string),
            start_time: occurred_at_ms,
            last_updated_at: occurred_at_ms,
            end_time: None,
            kind,
            name: name.into_owned(),
            fields,
            outcome: None,
        };
        print_span_snapshot("begin", &state);
        self.spans
            .lock()
            .await
            .insert(span.span_id().to_string(), state);
    }

    async fn update_span(&self, occurred_at_ms: u64, span: &TraceSpanHandle, fields: Value) {
        let mut spans = self.spans.lock().await;
        let state = spans
            .get_mut(span.span_id())
            .unwrap_or_else(|| panic!("stdout backend update on unknown span: {}", span.span_id()));
        state.last_updated_at = occurred_at_ms;
        merge_json_fields(&mut state.fields, fields);
        print_span_snapshot("update", state);
    }

    async fn end_span(
        &self,
        occurred_at_ms: u64,
        span: TraceSpanHandle,
        outcome: TraceOutcome,
        fields: Value,
    ) {
        let mut spans = self.spans.lock().await;
        let state = spans
            .get_mut(span.span_id())
            .unwrap_or_else(|| panic!("stdout backend end on unknown span: {}", span.span_id()));
        state.last_updated_at = occurred_at_ms;
        state.end_time = Some(occurred_at_ms);
        state.outcome = Some(outcome);
        merge_json_fields(&mut state.fields, fields);
        print_span_snapshot("end", state);
        spans.remove(span.span_id());
    }

    async fn finalize_trace(
        &self,
        occurred_at_ms: u64,
        final_parent_span_id: Option<String>,
        outcome: TraceOutcome,
        fields: Value,
    ) {
        println!(
            "{}",
            json!({
                "record_type": "trace_finalization",
                "occurred_at_ms": occurred_at_ms,
                "final_parent_span_id": final_parent_span_id,
                "outcome": format!("{:?}", outcome),
                "fields": fields,
            })
        );
    }

    async fn force_finalize_trace(
        &self,
        occurred_at_ms: u64,
        final_parent_span_id: Option<String>,
        outcome: TraceOutcome,
        fields: Value,
    ) {
        println!(
            "{}",
            json!({
                "record_type": "trace_force_finalization",
                "occurred_at_ms": occurred_at_ms,
                "final_parent_span_id": final_parent_span_id,
                "outcome": format!("{:?}", outcome),
                "fields": fields,
            })
        );
    }
}

fn print_span_snapshot(operation: &str, state: &StdoutSpanState) {
    println!(
        "{}",
        json!({
            "record_type": "trace_span",
            "operation": operation,
            "span": {
                "span_id": state.span_id,
                "trace_id": state.trace_id,
                "parent_span_id": state.parent_span_id,
                "span_kind": format!("{:?}", state.kind),
                "name": state.name,
                "start_time": state.start_time,
                "last_updated_at": state.last_updated_at,
                "end_time": state.end_time,
                "outcome": state.outcome.map(|value| format!("{:?}", value)),
                "fields": state.fields,
            }
        })
    );
}

fn merge_json_fields(target: &mut Value, update: Value) {
    if let (Value::Object(target_map), Value::Object(update_map)) = (target, update) {
        for (key, value) in update_map {
            target_map.insert(key, value);
        }
    }
}
