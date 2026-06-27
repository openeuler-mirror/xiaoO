use crate::backend::BackendCheckpointRef;
use crate::gateway::{SessionLifecycleStatus, SessionRecord};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("session not found: {session_id}")]
    NotFound { session_id: String },
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn load(&self, session_id: &str) -> Option<SessionRecord>;
    async fn save(&self, record: SessionRecord);
    async fn delete(&self, session_id: &str) -> Option<SessionRecord>;
    async fn transition(
        &self,
        session_id: &str,
        status: SessionLifecycleStatus,
        last_error: Option<String>,
        updated_at_ms: u64,
    ) -> Result<SessionRecord, SessionStoreError>;

    async fn batch_mark_paused_due_to_eviction(
        &self,
        session_ids: &[String],
        checkpoint: Option<BackendCheckpointRef>,
    );

    /// Returns all sessions whose `parent_runtime_id` equals `parent_runtime_id`.
    /// Used by `force_close_session` to cascade-close descendants forked via
    /// `checkout`.
    async fn list_children(&self, parent_runtime_id: &str) -> Vec<SessionRecord>;
}

#[derive(Clone, Default)]
pub struct InMemorySessionStore {
    records: Arc<RwLock<HashMap<String, SessionRecord>>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn load(&self, session_id: &str) -> Option<SessionRecord> {
        self.records.read().await.get(session_id).cloned()
    }

    async fn save(&self, record: SessionRecord) {
        self.records
            .write()
            .await
            .insert(record.session_id.clone(), record);
    }

    async fn delete(&self, session_id: &str) -> Option<SessionRecord> {
        self.records.write().await.remove(session_id)
    }

    async fn transition(
        &self,
        session_id: &str,
        status: SessionLifecycleStatus,
        last_error: Option<String>,
        updated_at_ms: u64,
    ) -> Result<SessionRecord, SessionStoreError> {
        let mut records = self.records.write().await;
        let record = records
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound {
                session_id: session_id.to_string(),
            })?;
        record.status = status;
        record.last_error = last_error;
        record.updated_at_ms = updated_at_ms;
        Ok(record.clone())
    }

    async fn batch_mark_paused_due_to_eviction(
        &self,
        session_ids: &[String],
        checkpoint: Option<BackendCheckpointRef>,
    ) {
        let mut records = self.records.write().await;
        let now_ms = current_time_ms();

        for session_id in session_ids {
            if let Some(session) = records.get_mut(session_id) {
                session.status = SessionLifecycleStatus::Paused;
                session.backend_instance = None;
                session.paused_backend_checkpoint = checkpoint.clone();
                session.last_error = None;
                session.updated_at_ms = now_ms;
            }
        }
    }

    async fn list_children(&self, parent_runtime_id: &str) -> Vec<SessionRecord> {
        self.records
            .read()
            .await
            .values()
            .filter(|r| r.parent_runtime_id.as_deref() == Some(parent_runtime_id))
            .cloned()
            .collect()
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
