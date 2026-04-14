use super::backend_trait::TraceBackend;
use agent_contracts::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use async_trait::async_trait;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use tokio::sync::Mutex;

pub(crate) struct NoopBackend {
    spans: Mutex<HashMap<String, NoopSpanState>>,
}

struct NoopSpanState {
    trace_id: String,
    parent_span_id: Option<String>,
    start_time: u64,
    last_updated_at: u64,
    end_time: Option<u64>,
    kind: TraceSpanKind,
    name: String,
    fields: Value,
    outcome: Option<TraceOutcome>,
}

impl NoopBackend {
    pub(crate) fn new() -> Self {
        Self {
            spans: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl TraceBackend for NoopBackend {
    async fn begin_span(
        &self,
        occurred_at_ms: u64,
        span: &TraceSpanHandle,
        kind: TraceSpanKind,
        name: Cow<'static, str>,
        fields: Value,
    ) {
        self.spans.lock().await.insert(
            span.span_id().to_string(),
            NoopSpanState {
                trace_id: span.trace_id().to_string(),
                parent_span_id: span.parent_span_id().map(ToString::to_string),
                start_time: occurred_at_ms,
                last_updated_at: occurred_at_ms,
                end_time: None,
                kind,
                name: name.into_owned(),
                fields,
                outcome: None,
            },
        );
    }

    async fn update_span(&self, occurred_at_ms: u64, span: &TraceSpanHandle, fields: Value) {
        let mut spans = self.spans.lock().await;
        let state = spans
            .get_mut(span.span_id())
            .unwrap_or_else(|| panic!("noop backend update on unknown span: {}", span.span_id()));
        state.last_updated_at = occurred_at_ms;
        merge_json_fields(&mut state.fields, fields);
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
            .unwrap_or_else(|| panic!("noop backend end on unknown span: {}", span.span_id()));
        state.last_updated_at = occurred_at_ms;
        state.end_time = Some(occurred_at_ms);
        state.outcome = Some(outcome);
        let _ = (
            &state.trace_id,
            &state.parent_span_id,
            &state.start_time,
            &state.kind,
            &state.name,
        );
        merge_json_fields(&mut state.fields, fields);
        spans.remove(span.span_id());
    }

    async fn finalize_trace(&self, _occurred_at_ms: u64, _outcome: TraceOutcome, _fields: Value) {}

    async fn force_finalize_trace(
        &self,
        _occurred_at_ms: u64,
        _outcome: TraceOutcome,
        _fields: Value,
    ) {
    }
}

fn merge_json_fields(target: &mut Value, update: Value) {
    if let (Value::Object(target_map), Value::Object(update_map)) = (target, update) {
        for (key, value) in update_map {
            target_map.insert(key, value);
        }
    }
}
