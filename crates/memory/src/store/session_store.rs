use std::path::PathBuf;

use async_trait::async_trait;

use crate::{ensure_valid_session_id, invalid_session_id_io_error, SessionMemorySummary};

use super::fs_json::{read_json, write_json};

#[async_trait]
pub trait SessionMemoryStore: Send + Sync {
    async fn save_summary(&self, summary: &SessionMemorySummary) -> std::io::Result<()>;
    async fn load_summary(&self, session_id: &str) -> std::io::Result<SessionMemorySummary>;
}

pub struct FilesystemSessionMemoryStore {
    root: PathBuf,
}

impl FilesystemSessionMemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn summary_dir(&self) -> PathBuf {
        self.root.join("session")
    }

    fn summary_path(&self, session_id: &str) -> PathBuf {
        self.summary_dir().join(format!("{session_id}.json"))
    }
}

#[async_trait]
impl SessionMemoryStore for FilesystemSessionMemoryStore {
    async fn save_summary(&self, summary: &SessionMemorySummary) -> std::io::Result<()> {
        ensure_valid_session_id(&summary.session_id).map_err(|_| invalid_session_id_io_error())?;

        write_json(&self.summary_path(&summary.session_id), summary).await
    }

    async fn load_summary(&self, session_id: &str) -> std::io::Result<SessionMemorySummary> {
        ensure_valid_session_id(session_id).map_err(|_| invalid_session_id_io_error())?;
        read_json(&self.summary_path(session_id)).await
    }
}
