use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{
    ensure_valid_durable_memory_id, invalid_durable_memory_id_io_error, DurableMemory,
    DurableMemoryManifestEntry,
};

use super::fs_json::{list_json_files, read_json, write_json};

#[async_trait]
pub trait DurableMemoryStore: Send + Sync {
    async fn save_memory(&self, memory: &DurableMemory) -> std::io::Result<()>;
    async fn load_memory(&self, memory_id: &str) -> std::io::Result<DurableMemory>;
    async fn list_memories(&self) -> std::io::Result<Vec<DurableMemoryManifestEntry>>;
    async fn delete_memory(&self, memory_id: &str) -> std::io::Result<()>;
    async fn replace_all(
        &self,
        memories: &[DurableMemory],
    ) -> std::io::Result<Vec<DurableMemoryManifestEntry>>;
}

pub struct FilesystemDurableMemoryStore {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableStoreState {
    generation_id: String,
}

impl FilesystemDurableMemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn durable_dir(&self) -> PathBuf {
        self.root.join("durable")
    }

    fn durable_state_path(&self) -> PathBuf {
        self.durable_dir().join("current.json")
    }

    fn generations_dir(&self) -> PathBuf {
        self.durable_dir().join("generations")
    }

    fn generation_dir(&self, generation_id: &str) -> PathBuf {
        self.generations_dir().join(generation_id)
    }

    async fn active_memory_dir(&self) -> std::io::Result<PathBuf> {
        match fs::metadata(self.durable_state_path()).await {
            Ok(_) => {
                let state: DurableStoreState = read_json(&self.durable_state_path()).await?;
                Ok(self.generation_dir(&state.generation_id))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(self.durable_dir()),
            Err(error) => Err(error),
        }
    }

    async fn write_memory_to_path(
        &self,
        directory: PathBuf,
        memory: &DurableMemory,
    ) -> std::io::Result<()> {
        ensure_valid_durable_memory_id(&memory.memory_id)
            .map_err(|_| invalid_durable_memory_id_io_error())?;
        write_json(
            &directory.join(format!("{}.json", memory.memory_id)),
            memory,
        )
        .await
    }

    async fn write_state(&self, generation_id: String) -> std::io::Result<()> {
        write_json(
            &self.durable_state_path(),
            &DurableStoreState { generation_id },
        )
        .await
    }

    fn next_generation_id() -> std::io::Result<String> {
        let duration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let random_suffix: u32 = (duration.as_nanos() as u32).wrapping_mul(2654435761);
        Ok(format!(
            "swap-{}-{:08x}",
            duration.as_nanos(),
            random_suffix
        ))
    }

    async fn cleanup_old_generations(&self, keep_generation_id: &str) -> std::io::Result<()> {
        let generations_dir = self.generations_dir();
        let mut entries = match fs::read_dir(&generations_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            if name.to_string_lossy() != keep_generation_id {
                let _ = fs::remove_dir_all(entry.path()).await;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl DurableMemoryStore for FilesystemDurableMemoryStore {
    async fn save_memory(&self, memory: &DurableMemory) -> std::io::Result<()> {
        ensure_valid_durable_memory_id(&memory.memory_id)
            .map_err(|_| invalid_durable_memory_id_io_error())?;

        let active_dir = self.active_memory_dir().await?;
        self.write_memory_to_path(active_dir, memory).await
    }

    async fn load_memory(&self, memory_id: &str) -> std::io::Result<DurableMemory> {
        ensure_valid_durable_memory_id(memory_id)
            .map_err(|_| invalid_durable_memory_id_io_error())?;
        let active_dir = self.active_memory_dir().await?;
        read_json(&active_dir.join(format!("{memory_id}.json"))).await
    }

    async fn list_memories(&self) -> std::io::Result<Vec<DurableMemoryManifestEntry>> {
        let files = list_json_files(&self.active_memory_dir().await?).await?;
        let mut entries = Vec::new();
        for file in files {
            let memory: DurableMemory = read_json(&file).await?;
            entries.push(DurableMemoryManifestEntry {
                memory_id: memory.memory_id,
                kind: memory.kind,
                updated_at: memory.updated_at,
            });
        }
        entries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(entries)
    }

    async fn delete_memory(&self, memory_id: &str) -> std::io::Result<()> {
        ensure_valid_durable_memory_id(memory_id)
            .map_err(|_| invalid_durable_memory_id_io_error())?;
        let active_dir = self.active_memory_dir().await?;
        match fs::remove_file(active_dir.join(format!("{memory_id}.json"))).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn replace_all(
        &self,
        memories: &[DurableMemory],
    ) -> std::io::Result<Vec<DurableMemoryManifestEntry>> {
        let durable_dir = self.durable_dir();
        fs::create_dir_all(&durable_dir).await?;
        fs::create_dir_all(self.generations_dir()).await?;

        let generation_id = Self::next_generation_id()?;
        let staged_dir = self.generation_dir(&generation_id);
        for memory in memories {
            self.write_memory_to_path(staged_dir.clone(), memory)
                .await?;
        }

        self.write_state(generation_id.clone()).await?;
        let _ = self.cleanup_old_generations(&generation_id).await;

        self.list_memories().await
    }
}
