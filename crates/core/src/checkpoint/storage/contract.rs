use super::StorageConfig;
use crate::checkpoint::{state::StoredCheckpoint, CheckpointError, SnapshotId};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub(in crate::checkpoint) trait CheckpointStorage: Send + Sync {
    /// Permanently release storage resources when the owning session closes.
    fn close(&self);
    /// Save a checkpoint and return the ID evicted by the storage retention policy, if any.
    async fn save(
        &self,
        checkpoint: StoredCheckpoint,
    ) -> Result<Option<SnapshotId>, CheckpointError>;
    async fn load(&self, id: &SnapshotId) -> Result<Option<StoredCheckpoint>, CheckpointError>;
    async fn delete(&self, id: &SnapshotId) -> Result<(), CheckpointError>;
}

pub(in crate::checkpoint) trait StorageFactory: Send + Sync {
    fn create(&self, config: StorageConfig) -> Result<Arc<dyn CheckpointStorage>, CheckpointError>;
}
