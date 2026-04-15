use crate::{MoiraiError, Result, SpanStorage};

use super::agent_context::AgentContext;
use super::inner::PendingUpdate;

impl<S: SpanStorage + 'static> AgentContext<S> {
    pub async fn end_span(&self, span_id: &str) -> Result<()> {
        self.end_span_at(span_id, Self::current_time_ms()).await
    }

    pub async fn end_span_at(&self, span_id: &str, ended_at: i64) -> Result<()> {
        let mut inner = self.inner.lock().await;

        inner.open_spans.retain(|id| id != span_id);

        if let Some(buffered_span) = inner.buffer.iter_mut().find(|s| s.span_id == span_id) {
            buffered_span.last_updated_at = ended_at;
            buffered_span.end_time = Some(ended_at);
            return Ok(());
        }

        if let Some(pending_update) = inner.pending_updates.get_mut(span_id) {
            pending_update.last_updated_at = Some(ended_at);
            pending_update.end_time = Some(ended_at);
            return Ok(());
        }

        drop(inner);
        match self
            .storage
            .update_span_end(span_id, ended_at, ended_at)
            .await
        {
            Ok(()) => Ok(()),
            Err(MoiraiError::NotFound(_)) => {
                let mut inner = self.inner.lock().await;
                inner.pending_updates.insert(
                    span_id.to_string(),
                    PendingUpdate {
                        extras: serde_json::json!({}),
                        last_updated_at: Some(ended_at),
                        end_time: Some(ended_at),
                    },
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    pub async fn update_span_extras(
        &self,
        span_id: &str,
        extras: serde_json::Value,
        end_time: Option<i64>,
    ) -> Result<()> {
        self.update_span_extras_at(span_id, extras, Self::current_time_ms(), end_time)
            .await
    }

    pub async fn update_span_extras_at(
        &self,
        span_id: &str,
        extras: serde_json::Value,
        updated_at: i64,
        end_time: Option<i64>,
    ) -> Result<()> {
        let mut inner = self.inner.lock().await;

        if let Some(buffered_span) = inner.buffer.iter_mut().find(|s| s.span_id == span_id) {
            Self::merge_extras(&mut buffered_span.extras, &extras);
            buffered_span.last_updated_at = updated_at;
            if let Some(end_time_val) = end_time {
                buffered_span.end_time = Some(end_time_val);
            }
            return Ok(());
        }

        if let Some(pending_update) = inner.pending_updates.get_mut(span_id) {
            Self::merge_extras(&mut pending_update.extras, &extras);
            pending_update.last_updated_at = Some(updated_at);
            if let Some(end_time_val) = end_time {
                pending_update.end_time = Some(end_time_val);
            }
            return Ok(());
        }

        drop(inner);

        match self
            .storage
            .update_span_extras(span_id, extras.clone(), updated_at, end_time)
            .await
        {
            Ok(()) => Ok(()),
            Err(MoiraiError::NotFound(_)) => {
                let mut inner = self.inner.lock().await;
                let pending_update =
                    inner
                        .pending_updates
                        .entry(span_id.to_string())
                        .or_insert(PendingUpdate {
                            extras: serde_json::json!({}),
                            last_updated_at: None,
                            end_time: None,
                        });
                Self::merge_extras(&mut pending_update.extras, &extras);
                pending_update.last_updated_at = Some(updated_at);
                if let Some(end_time_val) = end_time {
                    pending_update.end_time = Some(end_time_val);
                }
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    pub async fn update_span(&self, span_id: &str, extras: serde_json::Value) -> Result<()> {
        self.update_span_at(span_id, extras, Self::current_time_ms())
            .await
    }

    pub async fn update_span_at(
        &self,
        span_id: &str,
        extras: serde_json::Value,
        updated_at: i64,
    ) -> Result<()> {
        self.update_span_extras_at(span_id, extras, updated_at, None)
            .await
    }

    /// Append a string value to a field in the span's extras.
    /// If the field doesn't exist, it's created with the chunk as its value.
    /// This is useful for accumulating streaming content.
    pub async fn append_span_field(&self, span_id: &str, field: &str, chunk: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;

        // Try to find and update in buffer first
        if let Some(buffered_span) = inner.buffer.iter_mut().find(|s| s.span_id == span_id) {
            let current = buffered_span
                .extras
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            buffered_span.extras[field] = serde_json::json!(format!("{}{}", current, chunk));
            buffered_span.last_updated_at = Self::current_time_ms();
            return Ok(());
        }

        // Try pending updates
        if let Some(pending_update) = inner.pending_updates.get_mut(span_id) {
            let current = pending_update
                .extras
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            pending_update.extras[field] = serde_json::json!(format!("{}{}", current, chunk));
            pending_update.last_updated_at = Some(Self::current_time_ms());
            return Ok(());
        }

        drop(inner);

        // For persisted spans, we need to read current value first
        let span = self.storage.get_span(span_id).await?;
        match span {
            Some(existing_span) => {
                let current = existing_span
                    .extras
                    .get(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new_value = format!("{}{}", current, chunk);
                let extras = serde_json::json!({ field: new_value });
                self.storage
                    .update_span_extras(span_id, extras, Self::current_time_ms(), None)
                    .await
            }
            None => {
                // Span doesn't exist yet, create pending update
                let mut inner = self.inner.lock().await;
                let pending_update =
                    inner
                        .pending_updates
                        .entry(span_id.to_string())
                        .or_insert(PendingUpdate {
                            extras: serde_json::json!({}),
                            last_updated_at: None,
                            end_time: None,
                        });
                pending_update.extras[field] = serde_json::json!(chunk);
                pending_update.last_updated_at = Some(Self::current_time_ms());
                Ok(())
            }
        }
    }
}
