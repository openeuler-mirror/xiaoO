use super::{
    memory::MemoryStorage, memory_cached_sqlite::MemoryCachedSqliteStorage, CheckpointStorage,
    StorageConfig, StorageFactory, StorageKind,
};
use crate::checkpoint::CheckpointError;
use std::sync::Arc;

pub(super) struct ConfiguredStorageFactory;

impl StorageFactory for ConfiguredStorageFactory {
    fn create(&self, config: StorageConfig) -> Result<Arc<dyn CheckpointStorage>, CheckpointError> {
        match config.kind {
            StorageKind::Memory => Ok(Arc::new(MemoryStorage::new(
                config.memory.unwrap_or_default(),
            ))),
            StorageKind::MemoryCachedSqlite => Ok(Arc::new(MemoryCachedSqliteStorage::new(
                config.memory_cached_sqlite.unwrap_or_default(),
            )?)),
        }
    }
}
