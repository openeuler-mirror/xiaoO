use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub kind: StorageKind,
    pub memory: Option<MemoryStorageConfig>,
    pub memory_cached_sqlite: Option<MemoryCachedSqliteStorageConfig>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    #[default]
    Memory,
    MemoryCachedSqlite,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryStorageConfig {
    /// Maximum retained snapshots, evicting the oldest saved ID first.
    /// None defaults to 20; zero retains no snapshots. Reads do not refresh order.
    pub capacity: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryCachedSqliteStorageConfig {
    pub path: Option<PathBuf>,
    pub compress: Option<String>,
    pub memory_cache_size: Option<usize>,
}
