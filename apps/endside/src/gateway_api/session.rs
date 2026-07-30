use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::backend::BackendManager;
use crate::chat::{FileChangeDelta, ToolExecutionUpdate};
use crate::gateway::{InMemorySessionStore, SessionControlPlane, SessionStore};
use crate::interaction_prompt::PromptRequest;

use agent_types::common::ids::AgentId;
use agent_types::events::LoopEndSummary;
use xiaoo_shared::plan::{SpawnSubagentMetadata, TodoSnapshotUpdate};

#[derive(Debug)]
pub enum SessionTurnUpdate {
    TurnStart {
        agent_id: AgentId,
        turn: u32,
    },
    SetAssistantContent {
        agent_id: AgentId,
        text: String,
    },
    SetAssistantThinking {
        agent_id: AgentId,
        text: String,
    },
    /// Incremental assistant content delta. Unlike `SetAssistantContent`
    /// (which replaces the entire message text), this variant only carries
    /// the new characters since the previous update; the TUI appends via
    /// `Message::append_content`, avoiding a full-string allocation per
    /// stream chunk.
    AppendAssistantContent {
        agent_id: AgentId,
        delta: String,
    },
    /// Incremental reasoning content delta. See `AppendAssistantContent`.
    AppendAssistantThinking {
        agent_id: AgentId,
        delta: String,
    },
    Tool {
        agent_id: AgentId,
        update: ToolExecutionUpdate,
    },
    /// Per-call file change delta forwarded from the daemon in remote mode.
    /// The TUI applies it directly to its session-diff tracker, bypassing
    /// the baseline/args computation that only the daemon can do (since it
    /// owns the filesystem where the tool ran).
    ToolFileChange {
        call_id: String,
        delta: FileChangeDelta,
    },
    /// Plan snapshot forwarded from the daemon in remote mode. The TUI
    /// applies it directly to `state.plan_state`, bypassing the
    /// `todo_write` args parsing that only the daemon can do (since the
    /// SSE `ToolResult` event strips `args_preview`).
    PlanUpdate {
        snapshot: TodoSnapshotUpdate,
    },
    /// Subagent lane metadata forwarded from the daemon in remote mode. The
    /// TUI creates/updates the subagent lane directly, bypassing the
    /// `spawn_subagent` args parsing that only the daemon can do.
    SubagentSpawn {
        metadata: SpawnSubagentMetadata,
    },
    LoopEnd {
        agent_id: AgentId,
        summary: LoopEndSummary,
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
    HookActions(Vec<agent_types::hook::HookAction>),
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
    pub(super) backend_manager: Arc<BackendManager>,
}

impl Default for SessionGateway {
    fn default() -> Self {
        let session_store = Arc::new(InMemorySessionStore::default());
        let backend_manager = Arc::new(BackendManager::new());

        // Start the cross-process signal handler so backends owned by this
        // process that another process has marked for eviction get evicted
        // immediately upon receiving SIGUSR1. Only spawn when a Tokio runtime
        // is available; unit tests that construct this type outside a runtime
        // don't need cross-process eviction.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let handler_store: Arc<dyn SessionStore> = session_store.clone();
            let handler_handle = backend_manager.clone().start_signal_handler(handler_store);
            handle.spawn(async move {
                handler_handle.await.ok();
            });
        }

        Self {
            session_store,
            lifecycle_control_plane: Arc::new(tokio::sync::Mutex::new(None)),
            active_session_ids: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            backend_manager,
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
