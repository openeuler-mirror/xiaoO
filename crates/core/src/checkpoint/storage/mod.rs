mod config;
mod contract;
mod factory;
mod memory;
mod memory_cached_sqlite;

pub use config::{
    MemoryCachedSqliteStorageConfig, MemoryStorageConfig, StorageConfig, StorageKind,
};
pub(super) use contract::{CheckpointStorage, StorageFactory};

pub(super) fn factory() -> impl StorageFactory {
    factory::ConfiguredStorageFactory
}
