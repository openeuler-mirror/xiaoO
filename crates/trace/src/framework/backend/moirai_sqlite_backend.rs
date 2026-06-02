use agent_contracts::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_types::common::BuildError;
use async_trait::async_trait;
use moirai::{AgentContext, ContextConfig, SqliteStorage};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Mutex as AsyncMutex;

use super::super::config::TraceRecorderConfig;
use super::backend_trait::TraceBackend;

static STORAGE_CACHE: LazyLock<Mutex<HashMap<String, Arc<SqliteStorage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) struct MoiraiSqliteBackend {
    context: AgentContext<SqliteStorage>,
    active_spans: AsyncMutex<HashSet<String>>,
    trace_finalized: AsyncMutex<bool>,
}

impl MoiraiSqliteBackend {
    pub(crate) async fn new(
        config: &TraceRecorderConfig,
        trace_id: String,
    ) -> Result<Self, BuildError> {
        let db_path_raw = config
            .db_path
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "db_path".to_string(),
            })?;
        let db_path = expand_tilde(db_path_raw);
        let agent_id = config
            .agent_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "agent_id".to_string(),
            })?;

        if let Some(parent) = Path::new(&db_path).parent() {
            std::fs::create_dir_all(parent).map_err(|error| BuildError::DependencyError {
                message: format!("failed to create trace db parent directory: {error}"),
            })?;
        }

        let storage = {
            let mut cache = STORAGE_CACHE.lock().map_err(|_| BuildError::DependencyError {
                message: "trace storage cache poisoned".to_string(),
            })?;
            cache
                .entry(db_path.clone())
                .or_insert_with(|| {
                    Arc::new(
                        SqliteStorage::new(&db_path).expect("failed to create shared moirai sqlite storage"),
                    )
                })
                .clone()
        };
        let context = AgentContext::new_user_with_explicit_trace(
            agent_id,
            trace_id,
            None,
            storage,
            ContextConfig {
                buffer_size: 1,
                immediate_flush: true,
            },
        )
        .await
        .map_err(|error| BuildError::DependencyError {
            message: format!("failed to create moirai trace context: {error}"),
        })?;

        Ok(Self {
            context,
            active_spans: AsyncMutex::new(HashSet::new()),
            trace_finalized: AsyncMutex::new(false),
        })
    }
}

#[async_trait]
impl TraceBackend for MoiraiSqliteBackend {
    async fn begin_span(
        &self,
        occurred_at_ms: u64,
        span: &TraceSpanHandle,
        kind: TraceSpanKind,
        name: Cow<'static, str>,
        fields: Value,
    ) {
        let persisted_span_id = self
            .context
            .record_span_at_with_parent(
                span_kind_to_moirai_type(kind),
                merge_json_fields(
                    fields,
                    serde_json::json!({
                        "name": name,
                        "trace_id": span.trace_id(),
                        "parent_span_id": span.parent_span_id(),
                    }),
                ),
                occurred_at_ms as i64,
                Some(span.span_id().to_string()),
                span.parent_span_id().map(ToString::to_string),
            )
            .await
            .unwrap_or_else(|error| panic!("moirai begin_span failed: {error}"));

        self.active_spans.lock().await.insert(persisted_span_id);
    }

    async fn update_span(&self, occurred_at_ms: u64, span: &TraceSpanHandle, fields: Value) {
        let active_spans = self.active_spans.lock().await;
        if !active_spans.contains(span.span_id()) {
            panic!(
                "moirai sqlite backend update on unknown span: {}",
                span.span_id()
            );
        }
        drop(active_spans);

        self.context
            .update_span_at(
                span.span_id(),
                merge_json_fields(
                    fields,
                    serde_json::json!({
                        "trace_id": span.trace_id(),
                        "parent_span_id": span.parent_span_id(),
                    }),
                ),
                occurred_at_ms as i64,
            )
            .await
            .unwrap_or_else(|error| panic!("moirai update_span failed: {error}"));
    }

    async fn end_span(
        &self,
        occurred_at_ms: u64,
        span: TraceSpanHandle,
        outcome: TraceOutcome,
        fields: Value,
    ) {
        let mut active_spans = self.active_spans.lock().await;
        if !active_spans.remove(span.span_id()) {
            panic!(
                "moirai sqlite backend end on unknown span: {}",
                span.span_id()
            );
        }
        drop(active_spans);

        self.context
            .update_span_at(
                span.span_id(),
                merge_json_fields(
                    fields,
                    serde_json::json!({
                        "outcome": format!("{:?}", outcome),
                        "trace_id": span.trace_id(),
                        "parent_span_id": span.parent_span_id(),
                    }),
                ),
                occurred_at_ms as i64,
            )
            .await
            .unwrap_or_else(|error| panic!("moirai end_span update failed: {error}"));

        self.context
            .end_span_at(span.span_id(), occurred_at_ms as i64)
            .await
            .unwrap_or_else(|error| panic!("moirai end_span failed: {error}"));
    }

    async fn finalize_trace(
        &self,
        _occurred_at_ms: u64,
        final_parent_span_id: Option<String>,
        outcome: TraceOutcome,
        fields: Value,
    ) {
        let mut finalized = self.trace_finalized.lock().await;
        if *finalized {
            return;
        }

        let active_spans = self.active_spans.lock().await;
        if !active_spans.is_empty() {
            let active_span_ids: Vec<String> = active_spans.iter().cloned().collect();
            panic!(
                "moirai trace finalization called while spans are still active: count={} active_span_ids={:?}",
                active_span_ids.len(),
                active_span_ids
            );
        }
        drop(active_spans);

        let message = fields
            .get("message")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let success = matches!(outcome, TraceOutcome::Ok);

        self.context
            .end_with_parent(success, message.as_deref(), final_parent_span_id)
            .await
            .unwrap_or_else(|error| panic!("moirai trace finalization failed: {error}"));

        *finalized = true;
    }

    async fn force_finalize_trace(
        &self,
        _occurred_at_ms: u64,
        final_parent_span_id: Option<String>,
        outcome: TraceOutcome,
        fields: Value,
    ) {
        let mut finalized = self.trace_finalized.lock().await;
        if *finalized {
            return;
        }

        let active_spans = self.active_spans.lock().await;
        let active_span_ids: Vec<String> = active_spans.iter().cloned().collect();
        drop(active_spans);

        for span_id in &active_span_ids {
            let force_closed_fields = merge_json_fields(
                serde_json::json!({
                    "abnormal_closed": true,
                    "force_finalize": true,
                }),
                fields.clone(),
            );
            self.context
                .update_span_at(span_id, force_closed_fields, _occurred_at_ms as i64)
                .await
                .unwrap_or_else(|error| {
                    panic!("moirai force_finalize update failed for span {span_id}: {error}")
                });
            self.context
                .end_span_at(span_id, _occurred_at_ms as i64)
                .await
                .unwrap_or_else(|error| {
                    panic!("moirai force_finalize end failed for span {span_id}: {error}")
                });
        }

        let mut active_spans = self.active_spans.lock().await;
        active_spans.clear();
        drop(active_spans);

        let message = fields
            .get("message")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let success = matches!(outcome, TraceOutcome::Ok);

        self.context
            .end_with_parent(success, message.as_deref(), final_parent_span_id)
            .await
            .unwrap_or_else(|error| panic!("moirai force trace finalization failed: {error}"));

        *finalized = true;
    }
}

fn span_kind_to_moirai_type(kind: TraceSpanKind) -> &'static str {
    match kind {
        TraceSpanKind::Turn => "TURN",
        TraceSpanKind::PromptBuild => "PROMPT_BUILD",
        TraceSpanKind::Compression => "COMPRESSION",
        TraceSpanKind::LlmCall => "LLM_CALL",
        TraceSpanKind::ToolCall => "TOOL_CALL",
        TraceSpanKind::Hook => "HOOK",
        TraceSpanKind::Custom => "CUSTOM",
    }
}

/// Expand `~` to home directory in path string.
fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

fn merge_json_fields(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base_map), Value::Object(extra_map)) => {
            for (key, value) in extra_map {
                base_map.insert(key, value);
            }
            Value::Object(base_map)
        }
        (base_value, Value::Object(extra_map)) => {
            let mut merged = serde_json::Map::new();
            merged.insert("value".to_string(), base_value);
            for (key, value) in extra_map {
                merged.insert(key, value);
            }
            Value::Object(merged)
        }
        (base_value, _) => base_value,
    }
}
