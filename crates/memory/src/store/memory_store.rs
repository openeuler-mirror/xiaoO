use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ensure_valid_session_id, invalid_session_id_io_error, MemorySnapshot};

use super::fs_json::{list_json_files, read_json, write_json};

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn save_snapshot(&self, snapshot: &MemorySnapshot) -> std::io::Result<()>;
    async fn load_snapshot(&self, session_id: &str) -> std::io::Result<MemorySnapshot>;
    async fn list_snapshots(&self) -> std::io::Result<Vec<MemoryIndexEntry>>;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryIndexEntry {
    pub session_id: String,
    pub updated_at: u64,
}

pub struct FilesystemMemoryStore {
    root: PathBuf,
}

impl FilesystemMemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn snapshot_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn snapshot_path(&self, session_id: &str) -> PathBuf {
        self.snapshot_dir().join(format!("{session_id}.json"))
    }
}

#[async_trait]
impl MemoryStore for FilesystemMemoryStore {
    async fn save_snapshot(&self, snapshot: &MemorySnapshot) -> std::io::Result<()> {
        ensure_valid_session_id(&snapshot.session_id).map_err(|_| invalid_session_id_io_error())?;

        write_json(&self.snapshot_path(&snapshot.session_id), snapshot).await
    }

    async fn load_snapshot(&self, session_id: &str) -> std::io::Result<MemorySnapshot> {
        ensure_valid_session_id(session_id).map_err(|_| invalid_session_id_io_error())?;
        let mut snapshot: MemorySnapshot = read_json(&self.snapshot_path(session_id)).await?;
        snapshot.rebuild_conversation();
        Ok(snapshot)
    }

    async fn list_snapshots(&self) -> std::io::Result<Vec<MemoryIndexEntry>> {
        let files = list_json_files(&self.snapshot_dir()).await?;
        let mut entries = Vec::new();
        for file in files {
            let snapshot: MemorySnapshot = read_json(&file).await?;
            entries.push(MemoryIndexEntry {
                session_id: snapshot.session_id,
                updated_at: snapshot.updated_at,
            });
        }
        entries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(entries)
    }
}

#[allow(dead_code)]
fn _ensure_path_types(_path: &Path) {}
