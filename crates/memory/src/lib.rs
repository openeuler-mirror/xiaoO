//! 会话工作记忆快照:MemoryManager 每轮把 loop_state 的消息同步进
//! MemorySnapshot,随 SessionRecord 持久化。本 crate 只负责快照的构建与
//! 同步,不承载摘要压缩、长期记忆或语义检索。

pub mod manager;
pub mod snapshot;

pub use manager::MemoryManager;
pub use snapshot::MemorySnapshot;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("invalid session id")]
    InvalidSessionId,
}

pub type MemoryResult<T> = Result<T, MemoryError>;

pub(crate) fn ensure_valid_session_id(session_id: &str) -> MemoryResult<()> {
    if is_safe_storage_key(session_id) {
        Ok(())
    } else {
        Err(MemoryError::InvalidSessionId)
    }
}

fn is_safe_storage_key(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut components = std::path::Path::new(trimmed).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(component)), None) if component == std::ffi::OsStr::new(trimmed)
    )
}
