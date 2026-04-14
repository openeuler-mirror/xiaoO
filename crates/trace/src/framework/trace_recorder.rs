use std::borrow::Cow;

use agent_contracts::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
use agent_types::common::BuildError;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use ulid::Ulid;

use super::backend::{
    MoiraiSqliteBackend, NoopBackend, StdoutBackend, TraceBackend, TraceBackendType,
};
use super::config::TraceRecorderConfig;

pub(crate) struct TraceRecorderImpl {
    trace_id: String,
    head_span_id: Mutex<Option<String>>,
    span_parent_index: Mutex<HashMap<String, Option<String>>>,
    backend: Box<dyn TraceBackend>,
}

impl TraceRecorderImpl {
    pub(crate) async fn new(config: &TraceRecorderConfig) -> Result<Self, BuildError> {
        let backend_type = TraceBackendType::parse(config.storage_backend.as_str())?;
        let backend: Box<dyn TraceBackend> = match backend_type {
            TraceBackendType::Noop => Box::new(NoopBackend::new()),
            TraceBackendType::Stdout => Box::new(StdoutBackend::new()),
            TraceBackendType::MoiraiSqlite => Box::new(MoiraiSqliteBackend::new(config).await?),
        };

        Ok(Self {
            trace_id: Ulid::new().to_string(),
            head_span_id: Mutex::new(None),
            span_parent_index: Mutex::new(HashMap::new()),
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
        let parent_span_id = self.head_span_id.lock().await.clone();
        let span_id = Ulid::new().to_string();
        let span = TraceSpanHandle::new(
            self.trace_id.clone(),
            span_id.clone(),
            parent_span_id.clone(),
        );

        self.backend
            .begin_span(current_time_ms(), &span, kind, name, fields)
            .await;

        self.span_parent_index
            .lock()
            .await
            .insert(span_id.clone(), parent_span_id);
        *self.head_span_id.lock().await = Some(span_id);

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

        let parent_span_id = self
            .span_parent_index
            .lock()
            .await
            .remove(span.span_id())
            .flatten();
        let mut head_span_id = self.head_span_id.lock().await;
        if head_span_id.as_deref() == Some(span.span_id()) {
            *head_span_id = parent_span_id;
        }
    }

    async fn finalize_trace(&self, outcome: TraceOutcome, fields: Value) {
        self.backend
            .finalize_trace(current_time_ms(), outcome, fields)
            .await
    }

    async fn force_finalize_trace(&self, outcome: TraceOutcome, fields: Value) {
        self.backend
            .force_finalize_trace(current_time_ms(), outcome, fields)
            .await
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}
