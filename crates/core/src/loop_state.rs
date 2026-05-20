use agent_types::compression::CompressionMeta;
use agent_types::outcome::TokenUsage;
use agent_types::ChatMessage;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::kvcache::KvCacheMap;

pub struct LoopState {
    pub session_id: uuid::Uuid,
    pub messages: Vec<ChatMessage>,
    pub turn_count: u32,
    pub token_usage: TokenUsage,
    pub compression_meta: CompressionMeta,
    pub kv_cache_map: KvCacheMap,
    pub cancel: CancellationToken,
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
        Self {
            session_id,
            messages: Vec::new(),
            turn_count: 0,
            token_usage: TokenUsage::default(),
            compression_meta: CompressionMeta::default(),
            kv_cache_map: KvCacheMap::default(),
            cancel: CancellationToken::new(),
        }
    }

    pub fn to_snapshot(&self) -> LoopStateSnapshot {
        LoopStateSnapshot {
            session_id: self.session_id,
            messages: self.messages.clone(),
            turn_count: self.turn_count,
            token_usage: self.token_usage.clone(),
            compression_meta: self.compression_meta.clone(),
            kv_cache_map: self.kv_cache_map.clone(),
        }
    }

    pub fn from_snapshot(snapshot: LoopStateSnapshot, cancel: CancellationToken) -> Self {
        Self {
            session_id: snapshot.session_id,
            messages: snapshot.messages,
            turn_count: snapshot.turn_count,
            token_usage: snapshot.token_usage,
            compression_meta: snapshot.compression_meta,
            kv_cache_map: snapshot.kv_cache_map,
            cancel,
        }
    }
}
