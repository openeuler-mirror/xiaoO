use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::remote::RemoteRuntimeConfig;
use crate::interaction_prompt::UserPromptResult;
use crate::session_gateway::{SessionGateway, SessionTurnUpdate};

pub(super) const STREAM_REVEAL_CHARS_PER_TICK: usize = 1;

pub(super) struct PendingStreamDone {
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) estimated_input_tokens: u64,
    pub(super) messages: Vec<llm_client::ChatMessage>,
}

pub struct GatewayRuntime {
    pub(super) session_gateway: SessionGateway,
    pub(super) stream_rx: Option<UnboundedReceiver<SessionTurnUpdate>>,
    pub(crate) stream_message_index: Option<usize>,
    pub(super) stream_reveal_buffer: String,
    pub(super) pending_stream_done: Option<PendingStreamDone>,
    pub(super) cancel_flag: Option<Arc<AtomicBool>>,
    pub(super) request_start: Option<Instant>,
    pub(super) first_token_latency_recorded: bool,
    pub(super) interaction_reply_tx: Option<UnboundedSender<UserPromptResult>>,
    pub(super) remote: Option<RemoteRuntimeConfig>,
    pub(super) remote_session_open: bool,
}

impl GatewayRuntime {
    pub fn new() -> Self {
        Self {
            session_gateway: SessionGateway::new(),
            stream_rx: None,
            stream_message_index: None,
            stream_reveal_buffer: String::new(),
            pending_stream_done: None,
            cancel_flag: None,
            request_start: None,
            first_token_latency_recorded: false,
            interaction_reply_tx: None,
            remote: None,
            remote_session_open: false,
        }
    }

    pub fn reset_for_new_session(&mut self, state: &mut crate::app_state::AppState) {
        if state.chat_state.is_loading
            || self.stream_rx.is_some()
            || self.pending_stream_done.is_some()
        {
            self.cancel_streaming(state);
        }
        self.stream_rx = None;
        self.stream_message_index = None;
        self.stream_reveal_buffer.clear();
        self.pending_stream_done = None;
        self.cancel_flag = None;
        self.request_start = None;
        self.first_token_latency_recorded = false;
        self.interaction_reply_tx = None;
        self.remote_session_open = false;
    }

    pub fn needs_active_refresh(&self) -> bool {
        self.stream_rx.is_some()
            || !self.stream_reveal_buffer.is_empty()
            || self.pending_stream_done.is_some()
    }

    pub async fn session_snapshot(
        &self,
        session_id: &str,
    ) -> Option<crate::gateway::SessionRecord> {
        self.session_gateway.session_snapshot(session_id).await
    }

    pub async fn import_session_snapshot(&self, record: crate::gateway::SessionRecord) {
        self.session_gateway.import_session_snapshot(record).await;
    }

    pub fn session_store_handle(&self) -> Arc<crate::gateway::InMemorySessionStore> {
        self.session_gateway.session_store.clone()
    }

    /// Closes all active sessions, firing the SessionClosed hook for each.
    /// Should be called before the application exits.
    pub async fn close_sessions(&mut self, session_id: &str) {
        self.close_remote_session(session_id).await;
        self.session_gateway.close_all_sessions().await;
    }
}
