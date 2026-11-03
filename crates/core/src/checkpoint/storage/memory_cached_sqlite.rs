use super::{CheckpointStorage, MemoryCachedSqliteStorageConfig};
use crate::checkpoint::{state::StoredCheckpoint, CheckpointError, SnapshotId};
use async_trait::async_trait;

/// Placeholder only: construction and storage operations are unsupported.
pub(super) struct MemoryCachedSqliteStorage;

impl MemoryCachedSqliteStorage {
    pub(super) fn new(_config: MemoryCachedSqliteStorageConfig) -> Result<Self, CheckpointError> {
        Err(CheckpointError::NotImplemented("MemoryCachedSqliteStorage"))
    }
}

#[async_trait]
impl CheckpointStorage for MemoryCachedSqliteStorage {
    fn close(&self) {
        // The existing close contract has no error return.
        tracing::warn!("Not implemented: MemoryCachedSqliteStorage::close");
    }

    async fn save(&self, _: StoredCheckpoint) -> Result<Option<SnapshotId>, CheckpointError> {
        Err(CheckpointError::NotImplemented(
            "MemoryCachedSqliteStorage::save",
        ))
    }

    async fn load(&self, _: &SnapshotId) -> Result<Option<StoredCheckpoint>, CheckpointError> {
        Err(CheckpointError::NotImplemented(
            "MemoryCachedSqliteStorage::load",
        ))
    }

    async fn delete(&self, _: &SnapshotId) -> Result<(), CheckpointError> {
        Err(CheckpointError::NotImplemented(
            "MemoryCachedSqliteStorage::delete",
        ))
    }
}
