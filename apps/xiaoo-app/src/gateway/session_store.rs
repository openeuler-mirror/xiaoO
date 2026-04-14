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
    async fn transition(
        &self,
        session_id: &str,
        status: SessionLifecycleStatus,
        last_error: Option<String>,
        updated_at_ms: u64,
    ) -> Result<SessionRecord, SessionStoreError>;
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
}
