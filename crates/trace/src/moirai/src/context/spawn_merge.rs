use crate::span_types;
use crate::{Result, Span, SpanStorage};

use super::agent_context::AgentContext;

pub struct SpawnResult<S: SpanStorage> {
    pub span_id: String,
    pub child_trace_id: String,
    pub child_context: AgentContext<S>,
}

impl<S: SpanStorage + 'static> AgentContext<S> {
    pub async fn spawn(&self, child_agent_id: &str, reason: Option<&str>) -> Result<SpawnResult<S>>
    where
        S: Clone,
    {
        let span_id = Self::generate_span_id();
        let now = Self::current_time_ms();

        let mut inner = self.inner.lock().await;
        if inner.ended {
            return Err(crate::MoiraiError::InvalidState(
                "cannot spawn after end()".to_string(),
            ));
        }
        let parent_trace_id = inner.trace_id.clone();
        let parent_span_id = inner.chronology_tail_span_id.clone();

        let child_trace_id = Self::generate_span_id();

        let spawn_span = Span {
            span_id: span_id.clone(),
            trace_id: inner.trace_id.clone(),
            parent_span_id: Some(inner.chronology_tail_span_id.clone()),
            span_type: span_types::SPAWN.to_string(),
            start_time: now,
            last_updated_at: now,
            end_time: Some(now),
            extras: serde_json::json!({
                "child_trace_id": child_trace_id,
                "child_agent_id": child_agent_id,
                "reason": reason
            }),
            created_at: now,
        };

        inner.buffer.push(spawn_span);
        inner.chronology_tail_span_id = span_id.clone();

        if inner.config.immediate_flush || inner.buffer.len() >= inner.config.buffer_size {
            let spans_to_flush: Vec<Span> = inner.buffer.drain(..).collect();
            drop(inner);
            for span in spans_to_flush {
                self.insert_span_with_pending_updates(span).await?;
            }
        } else {
            drop(inner);
        }

        let child_context = AgentContext::new_spawned(
            child_agent_id,
            &parent_trace_id,
            &parent_span_id,
            self.storage.clone(),
        )
        .await?;

        Ok(SpawnResult {
            span_id,
            child_trace_id: child_context.trace_id().await,
            child_context,
        })
    }

    pub async fn merge(&self, child_trace_id: &str, child_agent_id: &str) -> Result<String> {
        let current_head_span_id = {
            let inner = self.inner.lock().await;
            if inner.ended {
                return Err(crate::MoiraiError::InvalidState(
                    "cannot merge after end()".to_string(),
                ));
            }
            inner.chronology_tail_span_id.clone()
        };

        let child_last_span_id = self.storage.get_last_span_id(child_trace_id).await?;

        let mut parent_span_ids = vec![current_head_span_id.clone()];
        if let Some(span_id) = child_last_span_id {
            parent_span_ids.push(span_id);
        }

        let extras = serde_json::json!({
            "child_trace_id": child_trace_id,
            "child_agent_id": child_agent_id,
            "parent_span_ids": parent_span_ids
        });
        self.record_span_at_with_parent(
            span_types::MERGE,
            extras,
            Self::current_time_ms(),
            None,
            Some(current_head_span_id),
        )
        .await
    }

    pub async fn merge_multi(
        &self,
        children: &[(impl AsRef<str>, impl AsRef<str>)],
    ) -> Result<String> {
        let current_head_span_id = {
            let inner = self.inner.lock().await;
            if inner.ended {
                return Err(crate::MoiraiError::InvalidState(
                    "cannot merge after end()".to_string(),
                ));
            }
            inner.chronology_tail_span_id.clone()
        };

        let mut parent_span_ids = vec![current_head_span_id.clone()];
        for (child_trace_id, _) in children {
            if let Some(span_id) = self
                .storage
                .get_last_span_id(child_trace_id.as_ref())
                .await?
            {
                parent_span_ids.push(span_id);
            }
        }

        let child_trace_ids: Vec<&str> = children.iter().map(|(tid, _)| tid.as_ref()).collect();
        let child_agent_ids: Vec<&str> = children.iter().map(|(_, aid)| aid.as_ref()).collect();
        let extras = serde_json::json!({
            "child_trace_ids": child_trace_ids,
            "child_agent_ids": child_agent_ids,
            "parent_span_ids": parent_span_ids
        });
        self.record_span_at_with_parent(
            span_types::MERGE,
            extras,
            Self::current_time_ms(),
            None,
            Some(current_head_span_id),
        )
        .await
    }

    pub async fn merge_with_parents(
        &self,
        child_trace_id: &str,
        child_agent_id: &str,
        parent_span_ids: &[&str],
    ) -> Result<String> {
        let extras = serde_json::json!({
            "child_trace_id": child_trace_id,
            "child_agent_id": child_agent_id,
            "parent_span_ids": parent_span_ids
        });
        self.record_span(span_types::MERGE, extras).await
    }
}
