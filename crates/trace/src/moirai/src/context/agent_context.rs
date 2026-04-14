use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use ulid::Ulid;

use crate::span_types;
use crate::{Result, Span, SpanStorage};

use super::config::ContextConfig;
use super::inner::{Inner, PendingUpdate};

pub struct AgentContext<S: SpanStorage> {
    pub(super) inner: Arc<Mutex<Inner>>,
    pub(super) storage: Arc<S>,
    pub(super) ended: Arc<AtomicBool>,
    pub(super) trace_id: Arc<String>,
}

impl<S: SpanStorage + 'static> Clone for AgentContext<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            storage: self.storage.clone(),
            ended: self.ended.clone(),
            trace_id: self.trace_id.clone(),
        }
    }
}

impl<S: SpanStorage + 'static> AgentContext<S> {
    pub fn generate_span_id() -> String {
        Ulid::new().to_string()
    }

    pub fn current_time_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    async fn create_with_root_span(
        agent_id: String,
        storage: Arc<S>,
        span_type: impl Into<String>,
        extras: serde_json::Value,
        config: ContextConfig,
    ) -> Result<Self> {
        let span_type = span_type.into();
        let span_id = Self::generate_span_id();
        let trace_id = span_id.clone();
        let now = Self::current_time_ms();

        let root_span = Span {
            span_id: span_id.clone(),
            trace_id: trace_id.clone(),
            parent_span_id: None,
            span_type,
            start_time: now,
            last_updated_at: now,
            end_time: None,
            extras,
            created_at: now,
        };

        storage.insert_span(&root_span).await?;

        let inner = Inner {
            trace_id: trace_id.clone(),
            agent_id,
            head_span_id: span_id.clone(),
            root_span_start_time: now,
            open_spans: vec![span_id],
            buffer: Vec::new(),
            pending_updates: std::collections::HashMap::new(),
            config,
            ended: false,
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            storage,
            ended: Arc::new(AtomicBool::new(false)),
            trace_id: Arc::new(trace_id),
        })
    }

    pub async fn new_user(agent_id: &str, storage: Arc<S>) -> Result<Self> {
        Self::new_user_with_config(agent_id, storage, ContextConfig::default()).await
    }

    pub async fn new_user_with_config(
        agent_id: &str,
        storage: Arc<S>,
        config: ContextConfig,
    ) -> Result<Self> {
        Self::create_with_root_span(
            agent_id.to_string(),
            storage,
            span_types::USER,
            serde_json::json!({}),
            config,
        )
        .await
    }

    pub async fn new_quest(agent_id: &str, quest_id: &str, storage: Arc<S>) -> Result<Self> {
        Self::new_quest_with_config(agent_id, quest_id, storage, ContextConfig::default()).await
    }

    pub async fn new_quest_with_config(
        agent_id: &str,
        quest_id: &str,
        storage: Arc<S>,
        config: ContextConfig,
    ) -> Result<Self> {
        Self::create_with_root_span(
            agent_id.to_string(),
            storage,
            span_types::QUEST,
            serde_json::json!({ "quest_id": quest_id }),
            config,
        )
        .await
    }

    pub async fn new_spawned(
        agent_id: &str,
        parent_trace_id: &str,
        parent_span_id: &str,
        storage: Arc<S>,
    ) -> Result<Self> {
        Self::new_spawned_with_config(
            agent_id,
            parent_trace_id,
            parent_span_id,
            storage,
            ContextConfig::default(),
        )
        .await
    }

    pub async fn new_spawned_with_config(
        agent_id: &str,
        parent_trace_id: &str,
        parent_span_id: &str,
        storage: Arc<S>,
        config: ContextConfig,
    ) -> Result<Self> {
        Self::create_with_root_span(
            agent_id.to_string(),
            storage,
            span_types::SPAWNED,
            serde_json::json!({
                "parent_trace_id": parent_trace_id,
                "parent_span_id": parent_span_id
            }),
            config,
        )
        .await
    }

    pub async fn trace_id(&self) -> String {
        self.inner.lock().await.trace_id.clone()
    }

    pub async fn head_span_id(&self) -> String {
        self.inner.lock().await.head_span_id.clone()
    }

    pub(super) fn merge_extras(target: &mut serde_json::Value, update: &serde_json::Value) {
        if let (serde_json::Value::Object(target_obj), serde_json::Value::Object(update_obj)) =
            (target, update)
        {
            for (key, value) in update_obj {
                target_obj.insert(key.clone(), value.clone());
            }
        }
    }

    pub(super) async fn apply_pending_update(
        &self,
        span_id: &str,
        pending_update: PendingUpdate,
    ) -> Result<()> {
        self.storage
            .update_span_extras(
                span_id,
                pending_update.extras,
                pending_update
                    .last_updated_at
                    .unwrap_or_else(Self::current_time_ms),
                pending_update.end_time,
            )
            .await
    }

    pub(super) async fn insert_span_with_pending_updates(&self, span: Span) -> Result<()> {
        self.storage.insert_span(&span).await?;

        let pending_update = {
            let mut inner = self.inner.lock().await;
            inner.pending_updates.remove(&span.span_id)
        };

        if let Some(pending_update) = pending_update {
            self.apply_pending_update(&span.span_id, pending_update)
                .await?;
        }

        Ok(())
    }

    pub async fn record_span(
        &self,
        span_type: impl Into<String>,
        extras: serde_json::Value,
    ) -> Result<String> {
        self.record_span_at(span_type, extras, Self::current_time_ms(), None)
            .await
    }

    pub async fn record_span_at(
        &self,
        span_type: impl Into<String>,
        extras: serde_json::Value,
        started_at: i64,
        span_id: Option<String>,
    ) -> Result<String> {
        let span_type = span_type.into();
        let span_id = span_id.unwrap_or_else(Self::generate_span_id);

        let mut inner = self.inner.lock().await;

        let span = Span {
            span_id: span_id.clone(),
            trace_id: inner.trace_id.clone(),
            parent_span_id: Some(inner.head_span_id.clone()),
            span_type,
            start_time: started_at,
            last_updated_at: started_at,
            end_time: None,
            extras,
            created_at: started_at,
        };

        inner.buffer.push(span);
        inner.head_span_id = span_id.clone();
        inner.open_spans.push(span_id.clone());

        if inner.config.immediate_flush || inner.buffer.len() >= inner.config.buffer_size {
            let spans_to_flush: Vec<Span> = inner.buffer.drain(..).collect();
            drop(inner);
            for span in spans_to_flush {
                self.insert_span_with_pending_updates(span).await?;
            }
        }

        Ok(span_id)
    }
}

impl<S: SpanStorage> Drop for AgentContext<S> {
    fn drop(&mut self) {
        let is_last_reference = Arc::strong_count(&self.inner) == 1;
        let has_ended = self.ended.load(Ordering::SeqCst);

        if is_last_reference && !has_ended {
            eprintln!(
                "[moirai] WARNING: AgentContext dropped without calling end() \
                 (trace_id={}). Call ctx.end(success, message) before dropping.",
                self.trace_id.as_str()
            );
        }
    }
}
