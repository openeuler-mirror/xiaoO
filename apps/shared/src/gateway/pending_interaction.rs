use agent_types::interaction::{InteractionRequest, InteractionResponse};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::{oneshot, RwLock};

pub struct PendingInteraction {
    pub request: InteractionRequest,
    pub response_tx: oneshot::Sender<InteractionResponse>,
    pub created_at: Instant,
}

/// Shared store for pending interaction requests, keyed by session_id.
///
/// When a channel (e.g. Feishu) tool calls ask_user_question, the question is
/// sent to the user and a PendingInteraction is registered here.  The next
/// inbound message for the same session_id is then routed to resolve the
/// pending interaction instead of starting a new turn.
#[derive(Default)]
pub struct PendingInteractionStore {
    pending: RwLock<HashMap<String, PendingInteraction>>,
}

impl PendingInteractionStore {
    pub fn new() -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
        }
    }

    /// Register a pending interaction for the given session.
    pub async fn register(&self, session_id: &str, interaction: PendingInteraction) {
        self.pending
            .write()
            .await
            .insert(session_id.to_string(), interaction);
    }

    /// Take (remove) a pending interaction for the given session, if any.
    pub async fn take(&self, session_id: &str) -> Option<PendingInteraction> {
        self.pending.write().await.remove(session_id)
    }

    /// Check whether a pending interaction exists for the given session.
    pub async fn has_pending(&self, session_id: &str) -> bool {
        self.pending.read().await.contains_key(session_id)
    }
}
