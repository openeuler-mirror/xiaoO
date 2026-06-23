use agent_types::compression::CompressionMeta;
use agent_types::outcome::TokenUsage;
use agent_types::ChatMessage;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::kvcache::KvCacheMap;

pub struct LoopState {
    pub session_id: uuid::Uuid,
    pub messages: Arc<RwLock<Vec<ChatMessage>>>,
    pub turn_count: u32,
    pub token_usage: TokenUsage,
    pub compression_meta: CompressionMeta,
    pub kv_cache_map: KvCacheMap,
    pub cancel: CancellationToken,
    pub plan_nudged: bool,
    pub last_failure_sig: Option<u64>,
    pub repeated_failure_count: u32,
    pub last_success_sig: Option<u64>,
    pub repeated_success_count: u32,
    pub tool_executed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoopStateSnapshot {
    pub session_id: uuid::Uuid,
    pub messages: Vec<ChatMessage>,
    pub turn_count: u32,
    pub token_usage: TokenUsage,
    pub compression_meta: CompressionMeta,
    pub kv_cache_map: KvCacheMap,
}

impl LoopState {
    pub fn new(session_id: uuid::Uuid) -> Self {
        Self::new_with_cancel(session_id, CancellationToken::new())
    }

    pub fn new_with_cancel(session_id: uuid::Uuid, cancel: CancellationToken) -> Self {
        Self {
            session_id,
            messages: Arc::new(RwLock::new(Vec::new())),
            turn_count: 0,
            token_usage: TokenUsage::default(),
            compression_meta: CompressionMeta::default(),
            kv_cache_map: KvCacheMap::default(),
            cancel,
            plan_nudged: false,
            last_failure_sig: None,
            repeated_failure_count: 0,
            last_success_sig: None,
            repeated_success_count: 0,
            tool_executed: false,
        }
    }

    pub fn to_snapshot(&self) -> LoopStateSnapshot {
        LoopStateSnapshot {
            session_id: self.session_id,
            messages: self.messages.read().clone(),
            turn_count: self.turn_count,
            token_usage: self.token_usage.clone(),
            compression_meta: self.compression_meta.clone(),
            kv_cache_map: self.kv_cache_map.clone(),
        }
    }

    pub fn from_snapshot(snapshot: LoopStateSnapshot, cancel: CancellationToken) -> Self {
        Self {
            session_id: snapshot.session_id,
            messages: Arc::new(RwLock::new(snapshot.messages)),
            turn_count: snapshot.turn_count,
            token_usage: snapshot.token_usage,
            compression_meta: snapshot.compression_meta,
            kv_cache_map: snapshot.kv_cache_map,
            cancel,
            plan_nudged: false,
            last_failure_sig: None,
            repeated_failure_count: 0,
            last_success_sig: None,
            repeated_success_count: 0,
            tool_executed: false,
        }
    }

    /// Get a clone of the shared message storage Arc.
    pub fn messages_arc(&self) -> Arc<RwLock<Vec<ChatMessage>>> {
        Arc::clone(&self.messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_cancel_uses_provided_token() {
        let cancel = CancellationToken::new();
        let state = LoopState::new_with_cancel(uuid::Uuid::new_v4(), cancel.clone());

        cancel.cancel();

        assert!(state.cancel.is_cancelled());
    }
}
