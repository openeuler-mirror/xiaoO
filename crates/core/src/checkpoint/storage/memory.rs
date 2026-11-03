use super::{CheckpointStorage, MemoryStorageConfig};
use crate::checkpoint::{state::StoredCheckpoint, CheckpointError, SnapshotId};
use async_trait::async_trait;
use indexmap::IndexMap;
use parking_lot::RwLock;

pub(super) struct MemoryStorage {
    checkpoints: RwLock<Option<IndexMap<SnapshotId, StoredCheckpoint>>>,
    capacity: usize,
}
impl MemoryStorage {
    pub(super) fn new(config: MemoryStorageConfig) -> Self {
        Self {
            checkpoints: RwLock::new(Some(IndexMap::new())),
            capacity: config.capacity.unwrap_or(20),
        }
    }
}
#[async_trait]
impl CheckpointStorage for MemoryStorage {
    fn close(&self) {
        self.checkpoints.write().take();
    }
    async fn save(
        &self,
        checkpoint: StoredCheckpoint,
    ) -> Result<Option<SnapshotId>, CheckpointError> {
        let mut guard = self.checkpoints.write();
        let checkpoints = guard.as_mut().ok_or(CheckpointError::QueueClosed)?;
        // Replacing an existing ID preserves its original FIFO position.
        checkpoints.insert(checkpoint.snapshot.id.clone(), checkpoint);
        if checkpoints.len() > self.capacity {
            return Ok(checkpoints.shift_remove_index(0).map(|(id, _)| id));
        }
        Ok(None)
    }
    async fn load(&self, id: &SnapshotId) -> Result<Option<StoredCheckpoint>, CheckpointError> {
        Ok(self
            .checkpoints
            .read()
            .as_ref()
            .ok_or(CheckpointError::QueueClosed)?
            .get(id)
            .cloned())
    }
    async fn delete(&self, id: &SnapshotId) -> Result<(), CheckpointError> {
        self.checkpoints
            .write()
            .as_mut()
            .ok_or(CheckpointError::QueueClosed)?
            .shift_remove(id);
        Ok(())
    }
}
