use crate::backend::BackendCheckpointRef;
use crate::gateway::{SessionLifecycleStatus, SessionRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeRecord {
    pub runtime_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub status: SessionLifecycleStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl RuntimeRecord {
    pub(crate) fn from_session(session: &SessionRecord) -> Self {
        Self {
            runtime_id: session.session_id.clone(),
            conversation_id: session.conversation_id.clone(),
            sender_id: session.sender_id.clone(),
            status: session.status.clone(),
            created_at_ms: session.created_at_ms,
            updated_at_ms: session.updated_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckpointRequest {
    pub runtime_id: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckpointResult {
    pub checkpoint_id: String,
    pub runtime: RuntimeRecord,
    #[serde(default)]
    pub parent_checkpoint_id: Option<String>,
    pub created_at_ms: u64,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckoutRequest {
    pub checkpoint_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub sender_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckoutResult {
    pub checkpoint_id: String,
    pub source_runtime_id: String,
    pub runtime: RuntimeRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckpointSnapshotDeleteRequest {
    pub checkpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckpointSnapshotDeleteResult {
    pub checkpoint_id: String,
    pub runtime_id: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub provider_snapshot_id: Option<String>,
    #[serde(default)]
    pub provider_snapshot_names: Vec<String>,
    pub deleted_provider_snapshot: bool,
    pub deleted_at_ms: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RuntimeCheckpoint {
    pub checkpoint_id: String,
    pub runtime_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub session: SessionRecord,
    pub backend_checkpoint: Option<BackendCheckpointRef>,
    pub created_at_ms: u64,
    pub metadata: Value,
    pub name: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct InMemoryRuntimeCheckpointStore {
    state: Arc<RwLock<RuntimeCheckpointStoreState>>,
}

#[derive(Default)]
struct RuntimeCheckpointStoreState {
    checkpoints: HashMap<String, RuntimeCheckpoint>,
    runtime_heads: HashMap<String, String>,
}

impl InMemoryRuntimeCheckpointStore {
    pub(crate) async fn latest_for_runtime(&self, runtime_id: &str) -> Option<String> {
        self.state
            .read()
            .await
            .runtime_heads
            .get(runtime_id)
            .cloned()
    }

    pub(crate) async fn save(&self, checkpoint: RuntimeCheckpoint) {
        let mut state = self.state.write().await;
        state.runtime_heads.insert(
            checkpoint.runtime_id.clone(),
            checkpoint.checkpoint_id.clone(),
        );
        state
            .checkpoints
            .insert(checkpoint.checkpoint_id.clone(), checkpoint);
    }

    pub(crate) async fn load(&self, checkpoint_id: &str) -> Option<RuntimeCheckpoint> {
        self.state
            .read()
            .await
            .checkpoints
            .get(checkpoint_id)
            .cloned()
    }

    pub(crate) async fn register_runtime_head(&self, runtime_id: String, checkpoint_id: String) {
        self.state
            .write()
            .await
            .runtime_heads
            .insert(runtime_id, checkpoint_id);
    }

    pub(crate) async fn clear_backend_snapshot(
        &self,
        checkpoint_id: &str,
    ) -> Option<RuntimeCheckpoint> {
        let mut state = self.state.write().await;
        let checkpoint = state.checkpoints.get_mut(checkpoint_id)?;
        if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.as_mut() {
            backend_checkpoint.provider_snapshot_id = None;
            backend_checkpoint.provider_snapshot_names.clear();
        }
        Some(checkpoint.clone())
    }
}
