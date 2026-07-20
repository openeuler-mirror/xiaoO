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
    /// Process identifier used by the daemon's lease guard. `None` for
    /// legacy / anonymous callers (lease bypass).
    #[serde(default)]
    pub client_id: Option<String>,
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
pub struct RuntimePauseRequest {
    pub runtime_id: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimePauseResult {
    pub runtime: RuntimeRecord,
    pub checkpoint_id: String,
    pub created_at_ms: u64,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeResumeRequest {
    pub runtime_id: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeResumeResult {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeExecRequest {
    pub runtime_id: String,
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeExecResult {
    pub stdout_base64: String,
    pub stderr_base64: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeReadFileRequest {
    pub runtime_id: String,
    pub path: String,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeReadFileResult {
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeWriteFileRequest {
    pub runtime_id: String,
    pub path: String,
    pub content_base64: String,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeWriteFileResult {
    pub path: String,
    pub created: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeCheckpoint {
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
pub struct InMemoryRuntimeCheckpointStore {
    state: Arc<RwLock<RuntimeCheckpointStoreState>>,
}

#[derive(Default)]
struct RuntimeCheckpointStoreState {
    checkpoints: HashMap<String, RuntimeCheckpoint>,
    runtime_heads: HashMap<String, String>,
    paused_runtime_heads: HashMap<String, String>,
}

impl InMemoryRuntimeCheckpointStore {
    pub async fn latest_for_runtime(&self, runtime_id: &str) -> Option<String> {
        self.state
            .read()
            .await
            .runtime_heads
            .get(runtime_id)
            .cloned()
    }

    pub async fn save(&self, checkpoint: RuntimeCheckpoint) {
        let mut state = self.state.write().await;
        state.runtime_heads.insert(
            checkpoint.runtime_id.clone(),
            checkpoint.checkpoint_id.clone(),
        );
        state
            .checkpoints
            .insert(checkpoint.checkpoint_id.clone(), checkpoint);
    }

    pub async fn load(&self, checkpoint_id: &str) -> Option<RuntimeCheckpoint> {
        self.state
            .read()
            .await
            .checkpoints
            .get(checkpoint_id)
            .cloned()
    }

    pub async fn register_runtime_head(&self, runtime_id: String, checkpoint_id: String) {
        self.state
            .write()
            .await
            .runtime_heads
            .insert(runtime_id, checkpoint_id);
    }

    pub async fn register_paused_runtime(&self, runtime_id: String, checkpoint_id: String) {
        self.state
            .write()
            .await
            .paused_runtime_heads
            .insert(runtime_id, checkpoint_id);
    }

    pub async fn paused_checkpoint_for_runtime(&self, runtime_id: &str) -> Option<String> {
        self.state
            .read()
            .await
            .paused_runtime_heads
            .get(runtime_id)
            .cloned()
    }

    pub async fn clear_paused_runtime(&self, runtime_id: &str) {
        self.state
            .write()
            .await
            .paused_runtime_heads
            .remove(runtime_id);
    }

    pub async fn clear_backend_snapshot(&self, checkpoint_id: &str) -> Option<RuntimeCheckpoint> {
        let mut state = self.state.write().await;
        let checkpoint = state.checkpoints.get_mut(checkpoint_id)?;
        if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.as_mut() {
            backend_checkpoint.provider_snapshot_id = None;
            backend_checkpoint.provider_snapshot_names.clear();
        }
        Some(checkpoint.clone())
    }

    /// Returns all checkpoints created by `runtime_id` (i.e. checkpoints whose
    /// `runtime_id` field equals `runtime_id`). Used by `force_close_session`
    /// to delete provider snapshots and clean up checkpoint records when a
    /// runtime is closed.
    pub async fn list_checkpoints_for_runtime(&self, runtime_id: &str) -> Vec<RuntimeCheckpoint> {
        self.state
            .read()
            .await
            .checkpoints
            .values()
            .filter(|c| c.runtime_id == runtime_id)
            .cloned()
            .collect()
    }

    /// Removes all in-memory tracking for `runtime_id`: its `runtime_heads`
    /// entry, its `paused_runtime_heads` entry, and every checkpoint record
    /// whose `runtime_id` equals `runtime_id`. Provider snapshots must be
    /// deleted separately via `delete_checkpoint_snapshot` before calling
    /// this if remote cleanup is desired.
    pub async fn remove_runtime(&self, runtime_id: &str) {
        let mut state = self.state.write().await;
        state.runtime_heads.remove(runtime_id);
        state.paused_runtime_heads.remove(runtime_id);
        state.checkpoints.retain(|_, c| c.runtime_id != runtime_id);
    }
}
