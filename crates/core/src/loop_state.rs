use agent_types::compression::CompressionMeta;
use agent_types::outcome::TokenUsage;
use agent_types::ChatMessage;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct LoopState {
    pub session_id: uuid::Uuid,
    pub messages: Arc<RwLock<Vec<ChatMessage>>>,
    pub turn_count: u32,
    pub token_usage: TokenUsage,
    pub compression_meta: CompressionMeta,
    pub cancel: CancellationToken,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoopStateSnapshot {
    pub session_id: uuid::Uuid,
    pub messages: Vec<ChatMessage>,
    pub turn_count: u32,
    pub token_usage: TokenUsage,
    pub compression_meta: CompressionMeta,
}

impl LoopState {
    pub fn new(session_id: uuid::Uuid) -> Self {
        Self {
            session_id,
            messages: Arc::new(RwLock::new(Vec::new())),
            turn_count: 0,
            token_usage: TokenUsage::default(),
            compression_meta: CompressionMeta::default(),
            cancel: CancellationToken::new(),
        }
    }

    pub fn to_snapshot(&self) -> LoopStateSnapshot {
        LoopStateSnapshot {
            session_id: self.session_id,
            messages: self.messages.read().clone(),
            turn_count: self.turn_count,
            token_usage: self.token_usage.clone(),
            compression_meta: self.compression_meta.clone(),
        }
    }

    pub fn from_snapshot(snapshot: LoopStateSnapshot, cancel: CancellationToken) -> Self {
        Self {
            session_id: snapshot.session_id,
            messages: Arc::new(RwLock::new(snapshot.messages)),
            turn_count: snapshot.turn_count,
            token_usage: snapshot.token_usage,
            compression_meta: snapshot.compression_meta,
            cancel,
        }
    }

    /// Get a clone of the shared message storage Arc.
    pub fn messages_arc(&self) -> Arc<RwLock<Vec<ChatMessage>>> {
        Arc::clone(&self.messages)
    }
}
