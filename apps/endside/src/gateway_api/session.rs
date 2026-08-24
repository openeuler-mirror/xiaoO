use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    watch,
};

use crate::backend::BackendManager;
use crate::chat::{FileChangeDelta, ToolExecutionUpdate};
use crate::gateway::{
    InMemorySessionStore, SessionControlPlane, SessionStore, TurnMemoryAutomation,
};
use crate::interaction_prompt::PromptRequest;
use crate::status_panel::MemoryStatus;

use xiaoo_api::chat::AgentId;
use xiaoo_api::events::LoopEndSummary;
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
    MemoryStatus(MemoryStatus),
    Done {
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<xiaoo_api::chat::ChatMessage>,
    },
    HookActions(Vec<xiaoo_api::chat::HookAction>),
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
    /// One MCP memory client is shared by all local TUI turns. The nested
    /// option distinguishes not-yet-initialized from a disabled/failed setup.
    pub(super) memory_automation:
        Arc<tokio::sync::Mutex<Option<Option<Arc<dyn TurnMemoryAutomation>>>>>,
    /// Latest RAM-A health receiver. Unlike a turn's stream receiver, this
    /// remains available after a turn finishes so background ingest failures
    /// can update the TUI immediately.
    pub(super) memory_health:
        Arc<Mutex<Option<watch::Receiver<crate::gateway::MemoryAutomationHealth>>>>,
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
            memory_automation: Arc::new(tokio::sync::Mutex::new(None)),
            memory_health: Arc::new(Mutex::new(None)),
            backend_manager,
        }
    }
}

pub(super) struct ChannelLoopEventSink {
    pub(super) updates_tx: UnboundedSender<SessionTurnUpdate>,
    pub(super) loop_summary: Arc<Mutex<Option<xiaoo_api::events::LoopEndSummary>>>,
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

#[cfg(test)]
mod tests {
    use super::SessionGateway;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use xiaoo_shared::gateway::memory_automation::{
        CompletedTurnIngest, MemoryAutomationError, RecallMemory, TurnMemoryContext,
    };
    use xiaoo_shared::gateway::TurnMemoryAutomation;

    struct ClosingAutomation(AtomicBool);

    #[async_trait]
    impl TurnMemoryAutomation for ClosingAutomation {
        async fn recall(
            &self,
            _context: &TurnMemoryContext,
        ) -> Result<Vec<RecallMemory>, MemoryAutomationError> {
            Ok(Vec::new())
        }

        async fn enqueue_ingest(
            &self,
            _ingest: CompletedTurnIngest,
        ) -> Result<(), MemoryAutomationError> {
            Ok(())
        }

        fn recall_token_budget(&self) -> usize {
            0
        }

        async fn close(&self) -> Result<(), MemoryAutomationError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn close_all_sessions_closes_cached_memory_automation() {
        let gateway = SessionGateway::new();
        let automation = Arc::new(ClosingAutomation(AtomicBool::new(false)));
        *gateway.memory_automation.lock().await =
            Some(Some(automation.clone() as Arc<dyn TurnMemoryAutomation>));

        gateway.close_all_sessions().await;

        assert!(automation.0.load(Ordering::SeqCst));
        assert!(gateway.memory_automation.lock().await.is_none());
    }
}

#[async_trait]
impl xiaoo_api::runtime::PendingUserMessageSource for ChannelPendingUserMessages {
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
