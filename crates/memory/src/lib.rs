pub mod chunker;
pub mod durable;
pub mod embedding;
pub mod manager;
pub mod recall;
pub mod session;
pub mod snapshot;
pub mod store;
pub mod structured;
pub mod vector;

pub use chunker::{chunk_markdown, Chunk};
pub use durable::{
    DurableMemory, DurableMemoryKind, DurableMemoryManager, DurableMemoryManifestEntry,
    DurableMemoryPolicy,
};
#[cfg(feature = "sqlite")]
pub use embedding::OpenAiEmbedding;
pub use embedding::{EmbeddingProvider, NoopEmbedding};
pub use manager::MemoryManager;
pub use recall::{RecallPacket, RecallQuery};
pub use session::{
    truncate_session_memory_summary, SessionMemoryBudgetingResult, SessionMemoryManager,
    SessionMemoryPolicy, SessionMemorySummary, SessionSummarySource,
};
pub use snapshot::{ContentBlock, ConversationMessage, MemoryRole, MemorySnapshot};
#[cfg(feature = "sqlite")]
pub use store::SqliteDurableMemoryStore;
pub use store::{
    DurableMemoryStore, FilesystemDurableMemoryStore, FilesystemMemoryStore,
    FilesystemSessionMemoryStore, MemoryIndexEntry, MemoryStore, ScoredMemory, SemanticMemoryStore,
    SemanticSearchQuery, SessionMemoryStore,
};
pub use structured::{
    FactMemory, InstructionMemory, PromptHistoryEntry, TaskMemory, TokenUsageBaseline,
};
pub use vector::{cosine_similarity, hybrid_merge};

use agent_types::LlmError;
use std::{
    ffi::OsStr,
    path::{Component, Path},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("invalid configuration: {message}")]
    InvalidConfiguration { message: String },
    #[error("session memory budget exhausted: {message}")]
    SessionMemoryBudgetExhausted { message: String },
    #[error("session memory summary parse failed: {message}")]
    SessionMemorySummaryParse { message: String },
    #[error("invalid session id")]
    InvalidSessionId,
    #[error("invalid durable memory id")]
    InvalidDurableMemoryId,
    #[error("current task must not be empty")]
    EmptyTask,
    #[error("fact key must not be empty")]
    EmptyFactKey,
    #[error("instruction source must not be empty")]
    EmptyInstructionSource,
    #[error("durable memory limit exceeded: configured={configured}, actual={actual}")]
    DurableMemoryLimitExceeded { configured: usize, actual: usize },
    #[error("embedding error: {message}")]
    Embedding { message: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),
}

pub type MemoryResult<T> = Result<T, MemoryError>;

pub(crate) fn ensure_valid_session_id(session_id: &str) -> MemoryResult<()> {
    if is_safe_storage_key(session_id) {
        Ok(())
    } else {
        Err(MemoryError::InvalidSessionId)
    }
}

pub(crate) fn ensure_valid_durable_memory_id(memory_id: &str) -> MemoryResult<()> {
    if is_safe_storage_key(memory_id) {
        Ok(())
    } else {
        Err(MemoryError::InvalidDurableMemoryId)
    }
}

pub(crate) fn invalid_session_id_io_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "session_id must be a safe file name",
    )
}

pub(crate) fn invalid_durable_memory_id_io_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "memory_id must be a safe file name",
    )
}

fn is_safe_storage_key(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut components = Path::new(trimmed).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(component)), None) if component == OsStr::new(trimmed)
    )
}
