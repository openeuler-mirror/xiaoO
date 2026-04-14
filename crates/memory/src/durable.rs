use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    ensure_valid_durable_memory_id, invalid_durable_memory_id_io_error, DurableMemoryStore,
    MemoryError, MemoryResult,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DurableMemoryKind {
    Preference,
    Constraint,
    Fact,
    Procedure,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableMemory {
    pub memory_id: String,
    pub kind: DurableMemoryKind,
    pub content: String,
    pub source: String,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableMemoryPolicy {
    pub max_memories: usize,
}

impl DurableMemoryPolicy {
    pub fn validate(&self) -> MemoryResult<()> {
        if self.max_memories == 0 {
            return Err(MemoryError::InvalidConfiguration {
                message: "durable memory max_memories must be greater than zero".to_string(),
            });
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableMemoryManifestEntry {
    pub memory_id: String,
    pub kind: DurableMemoryKind,
    pub updated_at: u64,
}

pub struct DurableMemoryManager {
    store: Arc<dyn DurableMemoryStore>,
    policy: DurableMemoryPolicy,
}

impl DurableMemoryManager {
    pub fn new(
        store: Arc<dyn DurableMemoryStore>,
        policy: DurableMemoryPolicy,
    ) -> MemoryResult<Self> {
        policy.validate()?;
        Ok(Self { store, policy })
    }

    pub async fn save_memory(&self, memory: &DurableMemory) -> MemoryResult<()> {
        validate_memory(memory)?;

        let manifest = self.store.list_memories().await?;
        let exists = manifest
            .iter()
            .any(|entry| entry.memory_id == memory.memory_id);
        let next_count = if exists {
            manifest.len()
        } else {
            manifest.len() + 1
        };

        if next_count > self.policy.max_memories {
            return Err(MemoryError::DurableMemoryLimitExceeded {
                configured: self.policy.max_memories,
                actual: next_count,
            });
        }

        self.store.save_memory(memory).await?;
        Ok(())
    }

    pub async fn load_memory(&self, memory_id: &str) -> std::io::Result<DurableMemory> {
        ensure_valid_durable_memory_id(memory_id)
            .map_err(|_| invalid_durable_memory_id_io_error())?;
        self.store.load_memory(memory_id).await
    }

    pub async fn list_memories(&self) -> std::io::Result<Vec<DurableMemoryManifestEntry>> {
        self.store.list_memories().await
    }

    pub async fn delete_memory(&self, memory_id: &str) -> std::io::Result<()> {
        ensure_valid_durable_memory_id(memory_id)
            .map_err(|_| invalid_durable_memory_id_io_error())?;
        self.store.delete_memory(memory_id).await
    }

    pub async fn replace_all(
        &self,
        memories: &[DurableMemory],
    ) -> MemoryResult<Vec<DurableMemoryManifestEntry>> {
        if memories.len() > self.policy.max_memories {
            return Err(MemoryError::DurableMemoryLimitExceeded {
                configured: self.policy.max_memories,
                actual: memories.len(),
            });
        }

        for memory in memories {
            validate_memory(memory)?;
        }

        Ok(self.store.replace_all(memories).await?)
    }

    pub async fn list_and_load(&self, limit: usize) -> std::io::Result<Vec<DurableMemory>> {
        let manifest = self.store.list_memories().await?;
        let mut memories = Vec::new();

        for entry in manifest.into_iter().take(limit) {
            memories.push(self.store.load_memory(&entry.memory_id).await?);
        }

        Ok(memories)
    }
}

fn validate_memory(memory: &DurableMemory) -> MemoryResult<()> {
    ensure_valid_durable_memory_id(&memory.memory_id)?;

    if memory.content.trim().is_empty() {
        return Err(MemoryError::InvalidConfiguration {
            message: "durable memory content must not be empty".to_string(),
        });
    }

    Ok(())
}
