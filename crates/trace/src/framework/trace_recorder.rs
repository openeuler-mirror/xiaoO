use std::borrow::Cow;

use agent_contracts::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
use agent_types::common::BuildError;
use async_trait::async_trait;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use ulid::Ulid;

use super::backend::{
    MoiraiSqliteBackend, NoopBackend, StdoutBackend, TraceBackend, TraceBackendType,
};
use super::config::TraceRecorderConfig;

pub(crate) struct TraceRecorderImpl {
    trace_id: String,
    chronology_state: Mutex<ChronologyState>,
    backend: Box<dyn TraceBackend>,
}

struct ChronologyState {
    tail_span_id: Option<String>,
    finalized: bool,
}

impl TraceRecorderImpl {
    pub(crate) async fn new(config: &TraceRecorderConfig) -> Result<Self, BuildError> {
        let trace_id = Ulid::new().to_string();
        let backend_type = TraceBackendType::parse(config.storage_backend.as_str())?;
        let backend: Box<dyn TraceBackend> = match backend_type {
            TraceBackendType::Noop => Box::new(NoopBackend::new()),
            TraceBackendType::Stdout => Box::new(StdoutBackend::new()),
            TraceBackendType::MoiraiSqlite => {
                Box::new(MoiraiSqliteBackend::new(config, trace_id.clone()).await?)
            }
            TraceBackendType::MoiraiSqlite => {
                Box::new(MoiraiSqliteBackend::new(config, trace_id.clone()).await?)
            }
        };

        Ok(Self {
            trace_id: trace_id.clone(),
            chronology_state: Mutex::new(ChronologyState {
                tail_span_id: Some(trace_id),
                finalized: false,
            }),
            backend,
        })
    }
}

#[async_trait]
impl TraceRecorder for TraceRecorderImpl {
    async fn begin_span(
        &self,
        kind: TraceSpanKind,
        name: Cow<'static, str>,
        fields: Value,
    ) -> TraceSpanHandle {
        let mut chronology_state = self.chronology_state.lock().await;
        if chronology_state.finalized {
            panic!("trace begin_span called after finalization");
        }

        let parent_span_id = chronology_state.tail_span_id.clone();
        let span_id = Ulid::new().to_string();
        let span = TraceSpanHandle::new(
            self.trace_id.clone(),
            span_id.clone(),
            parent_span_id.clone(),
        );

        self.backend
            .begin_span(current_time_ms(), &span, kind, name, fields)
            .await;

        chronology_state.tail_span_id = Some(span_id);

        span
    }

    async fn update_span(&self, span: &TraceSpanHandle, fields: Value) {
        self.backend
            .update_span(current_time_ms(), span, fields)
            .await
    }

    async fn end_span(&self, span: TraceSpanHandle, outcome: TraceOutcome, fields: Value) {
        self.backend
            .end_span(current_time_ms(), span.clone(), outcome, fields)
            .await;
    }

    async fn finalize_trace(&self, outcome: TraceOutcome, fields: Value) {
        let mut chronology_state = self.chronology_state.lock().await;
        if chronology_state.finalized {
            return;
        }
        let final_parent_span_id = chronology_state.tail_span_id.clone();
        chronology_state.finalized = true;
        self.backend
            .finalize_trace(current_time_ms(), final_parent_span_id, outcome, fields)
            .await
    }

    async fn force_finalize_trace(&self, outcome: TraceOutcome, fields: Value) {
        let mut chronology_state = self.chronology_state.lock().await;
        if chronology_state.finalized {
            return;
        }
        let final_parent_span_id = chronology_state.tail_span_id.clone();
        chronology_state.finalized = true;
        self.backend
            .force_finalize_trace(current_time_ms(), final_parent_span_id, outcome, fields)
            .await
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
