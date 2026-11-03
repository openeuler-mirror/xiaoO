use agent_types::ChatMessage;

use crate::{MemoryResult, MemorySnapshot};

/// 会话工作记忆的持有者:session_worker 每轮用它从 loop_state 同步消息,
/// 再由调用方把 [`MemorySnapshot`] 持久化进 SessionRecord。
pub struct MemoryManager {
    snapshot: MemorySnapshot,
}

impl MemoryManager {
    pub fn new(session_id: impl Into<String>, updated_at: u64) -> MemoryResult<Self> {
        Ok(Self {
            snapshot: MemorySnapshot::new(session_id, updated_at, Vec::new())?,
        })
    }

    pub fn from_snapshot(snapshot: MemorySnapshot) -> Self {
        Self { snapshot }
    }

    pub fn snapshot(&self) -> &MemorySnapshot {
        &self.snapshot
    }

    pub fn sync_from_loop_state(
        &mut self,
        messages: &[ChatMessage],
        updated_at: u64,
    ) -> &MemorySnapshot {
        self.snapshot.sync_messages(messages, updated_at);
        &self.snapshot
    }
}
