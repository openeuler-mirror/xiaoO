use crate::span_types;
use crate::{Result, Span, SpanStorage};

use super::agent_context::AgentContext;

impl<S: SpanStorage + 'static> AgentContext<S> {
    pub async fn end(&self, success: bool, message: Option<&str>) -> Result<String> {
        let span_id = Self::generate_span_id();
        let now = Self::current_time_ms();

        let (trace_id, head_span_id, root_span_start_time, open_spans_count) = {
            let inner = self.inner.lock().await;
            let open_spans_count = inner.open_spans.len() as u32;
            (
                inner.trace_id.clone(),
                inner.head_span_id.clone(),
                inner.root_span_start_time,
                open_spans_count,
            )
        };

        let total_spans = self.storage.count_spans(&trace_id).await.unwrap_or(0) as u32;
        let duration_ms = now - root_span_start_time;

        let extras = serde_json::json!({
            "success": success,
            "message": message,
            "total_spans": total_spans,
            "open_spans": open_spans_count,
            "duration_ms": duration_ms,
        });

        let end_span = Span {
            span_id: span_id.clone(),
            trace_id,
            parent_span_id: Some(head_span_id),
            span_type: span_types::END.to_string(),
            start_time: now,
            last_updated_at: now,
            end_time: Some(now),
            extras,
            created_at: now,
        };

        let mut inner = self.inner.lock().await;
        if inner.ended {
            return Err(crate::MoiraiError::InvalidState(
                "end() already called".to_string(),
            ));
        }
        inner.buffer.push(end_span);
        inner.head_span_id = span_id.clone();
        inner.ended = true;
        self.ended.store(true, std::sync::atomic::Ordering::SeqCst);

        let spans_to_flush: Vec<Span> = inner.buffer.drain(..).collect();
        drop(inner);

        for span in spans_to_flush {
            self.insert_span_with_pending_updates(span).await?;
        }

        Ok(span_id)
    }

    pub async fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let spans_to_flush: Vec<Span> = inner.buffer.drain(..).collect();
        drop(inner);

        for span in spans_to_flush {
            self.insert_span_with_pending_updates(span).await?;
        }

        Ok(())
    }
}
