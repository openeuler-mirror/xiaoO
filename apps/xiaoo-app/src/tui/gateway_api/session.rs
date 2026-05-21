use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::chat::ToolExecutionUpdate;
use crate::gateway::{InMemorySessionStore, SessionControlPlane};
use crate::interaction_prompt::PromptRequest;

use agent_types::common::ids::AgentId;

#[derive(Debug)]
pub enum SessionTurnUpdate {
    SetAssistantContent {
        agent_id: AgentId,
        text: String,
    },
    SetAssistantThinking {
        agent_id: AgentId,
        text: String,
    },
    Tool {
        _agent_id: AgentId,
        update: ToolExecutionUpdate,
    },
    InteractionPrompt(PromptRequest),
    PendingUserMessagesConsumed {
        prompts: Vec<String>,
    },
    Done {
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<llm_client::ChatMessage>,
    },
    Err(String),
}

#[derive(Clone)]
pub struct SessionGateway {
    pub(super) session_store: Arc<InMemorySessionStore>,
    /// Persistent control plane used solely for session lifecycle hooks.
    /// Initialized lazily on the first session open.
    pub(super) lifecycle_control_plane:
        Arc<tokio::sync::Mutex<Option<Arc<dyn SessionControlPlane>>>>,
    /// Session IDs that have been opened and not yet closed.
    pub(super) active_session_ids: Arc<tokio::sync::Mutex<HashSet<String>>>,
}

impl Default for SessionGateway {
    fn default() -> Self {
        Self {
            session_store: Arc::new(InMemorySessionStore::default()),
            lifecycle_control_plane: Arc::new(tokio::sync::Mutex::new(None)),
            active_session_ids: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }
    }
}

pub(super) struct ChannelLoopEventSink {
    pub(super) updates_tx: UnboundedSender<SessionTurnUpdate>,
    pub(super) loop_summary: Arc<Mutex<Option<agent_types::events::LoopEndSummary>>>,
}

pub(super) struct ChannelToolEventSink {
    pub(super) updates_tx: UnboundedSender<SessionTurnUpdate>,
}

pub(super) struct ChannelInteractionHandle {
    pub(super) updates_tx: UnboundedSender<SessionTurnUpdate>,
    pub(super) interaction_rx:
        tokio::sync::Mutex<UnboundedReceiver<crate::interaction_prompt::UserPromptResult>>,
}

pub(super) struct ChannelPendingUserMessages {
    pub(super) updates_tx: UnboundedSender<SessionTurnUpdate>,
    pub(super) pending: Arc<Mutex<VecDeque<String>>>,
}

impl ChannelPendingUserMessages {
    pub(super) fn new(
        updates_tx: UnboundedSender<SessionTurnUpdate>,
        pending: Arc<Mutex<VecDeque<String>>>,
    ) -> Self {
        Self {
            updates_tx,
            pending,
        }
    }
}

#[async_trait]
impl xiaoo_core::PendingUserMessageSource for ChannelPendingUserMessages {
    async fn drain_pending_user_messages(&self) -> Vec<String> {
        let prompts = self
            .pending
            .lock()
            .map(|mut pending| pending.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();

        if !prompts.is_empty() {
            let _ = self
                .updates_tx
                .send(SessionTurnUpdate::PendingUserMessagesConsumed {
                    prompts: prompts.clone(),
                });
        }

        prompts
    }
}
