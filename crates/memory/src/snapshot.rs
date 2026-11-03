use agent_types::ChatMessage;
use serde::{Deserialize, Serialize};

use crate::{ensure_valid_session_id, MemoryResult};

/// 随 SessionRecord 持久化的工作记忆快照,只承载 loop 的原始消息;
/// session_id 校验保证它可作为持久化记录的安全键。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemorySnapshot {
    pub session_id: String,
    pub updated_at: u64,
    pub messages: Vec<ChatMessage>,
}

impl MemorySnapshot {
    pub fn new(
        session_id: impl Into<String>,
        updated_at: u64,
        messages: Vec<ChatMessage>,
    ) -> MemoryResult<Self> {
        let session_id = session_id.into();
        ensure_valid_session_id(&session_id)?;

        Ok(Self {
            session_id,
            updated_at,
            messages,
        })
    }

    pub fn sync_messages(&mut self, messages: &[ChatMessage], updated_at: u64) {
        self.messages = messages.to_vec();
        self.updated_at = updated_at;
    }
}
