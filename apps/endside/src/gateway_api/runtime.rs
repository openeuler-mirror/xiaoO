use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use super::remote::RemoteRuntimeConfig;
use crate::gateway_api::http_timeouts::resolve_http_connect_timeout;
use crate::interaction_prompt::UserPromptResult;
use crate::session_gateway::{SessionGateway, SessionTurnUpdate};

pub(super) const STREAM_REVEAL_CHARS_PER_TICK: usize = 1;

/// Upper bound for [`GatewayRuntime::settle_in_flight_turn`] — how long a
/// caller (manual `/save`, interrupt auto-save) waits for an in-flight
/// (cancelled) turn to settle before reading the session store anyway.
/// Since the core-side cancel short-circuit (`llm_call` races the stream
/// against the cancel token and persists the partial message
/// immediately), a cancelled turn normally settles in milliseconds; the
/// timeout is the safety net for pathological backends (hung stream, a
/// non-cancelled turn settling naturally, layer-1-only cores).
pub(crate) const TURN_SETTLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Poll cadence of [`GatewayRuntime::settle_in_flight_turn`] while pumping
/// `poll_stream_updates` outside the normal event loop.
const TURN_SETTLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

pub(super) struct PendingStreamDone {
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) estimated_input_tokens: u64,
    pub(super) messages: Vec<xiaoo_api::chat::ChatMessage>,
}

pub struct GatewayRuntime {
    pub(super) session_gateway: SessionGateway,
    pub(super) stream_rx: Option<UnboundedReceiver<SessionTurnUpdate>>,
    pub(crate) stream_message_index: Option<usize>,
    pub(super) stream_reveal_buffer: String,
    pub(super) pending_stream_done: Option<PendingStreamDone>,
    /// Cancellation token shared with the backend turn. The TUI holds a clone
    /// so `cancel_streaming` can fire `.cancel()`; the original is passed into
    /// `spawn_turn` via `SessionRuntimeBindings` so the session actor uses it
    /// instead of creating its own token.
    pub(super) cancel_token: Option<CancellationToken>,
    /// True after Esc-cancel while the cancelled turn's stream is still
    /// connected: the backend only persists the partial loop state (and
    /// sends the terminal `Done`/`Err` update) after the in-flight LLM
    /// call returns. In this mode `poll_stream_updates` ignores all
    /// conversation-producing updates (TextDelta, tool events, …) so no
    /// ghost messages appear, and consumes only `Done`/`Err` — `Done`
    /// refreshes `session_messages` with the persisted partial state.
    /// Cleared when the drain ends, when a new turn starts, and on
    /// `reset_for_new_session`.
    pub(super) draining_after_cancel: bool,
    pub(super) request_start: Option<Instant>,
    pub(super) first_token_latency_recorded: bool,
    pub(super) interaction_reply_tx: Option<UnboundedSender<UserPromptResult>>,
    pub(super) pending_user_messages: Arc<Mutex<VecDeque<String>>>,
    pub(super) remote: Option<RemoteRuntimeConfig>,
    pub(super) remote_session_open: bool,
    /// Hook actions received from the daemon (via SSE `Done` event) that
    /// the TUI needs to execute (switch session, set title). Drained by the
    /// App's event loop after `poll_stream_updates` returns.
    pub(crate) pending_hook_actions: Vec<xiaoo_api::chat::HookAction>,
    /// Per-TUI-process identifier sent with every remote RPC body so the
    /// daemon can attribute the call to this TUI's attach lease. Mirrors
    /// `AppState.client_id`.
    pub(crate) client_id: String,
    /// Cached OS hostname read once at startup so per-RPC paths don't do a
    /// blocking `/etc/hostname` read on the async event loop. `None` if the
    /// hostname could not be resolved — purely informational.
    pub(crate) client_hostname: Option<String>,
    /// Shared `reqwest::Client` for all outbound HTTP. Only a *connect*
    /// timeout is configured — the SSE turn stream is a long-lived response
    /// body and must NOT be cut by a per-request timeout. Short RPCs
    /// additionally wrap `send()` in a `tokio::time::timeout`.
    pub(crate) http_client: reqwest::Client,
}

impl GatewayRuntime {
    /// Construct a new gateway runtime bound to the given `client_id`. The id
    /// is per-process (never persisted) so the daemon's attach-lease table
    /// can distinguish concurrent TUIs from the same user; read-only for the
    /// lifetime of the runtime.
    pub fn new(client_id: String) -> Self {
        // Build the shared HTTP client with a connect-phase timeout. `build()`
        // only fails on TLS backend init failure; in that case fall back to
        // `Client::new()` (no connect timeout) so the TUI still works, with
        // an `error!` so an operator can fix the TLS backend.
        let connect_timeout = resolve_http_connect_timeout();
        let http_client = match reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    "failed to build reqwest::Client with connect_timeout={connect_timeout:?}; \
                     retrying with default builder (connect-phase timeout is OFF — \
                     TUI may freeze up to the OS TCP timeout on an unreachable daemon)",
                );
                reqwest::Client::new()
            }
        };
        Self {
            session_gateway: SessionGateway::new(),
            stream_rx: None,
            stream_message_index: None,
            stream_reveal_buffer: String::new(),
            pending_stream_done: None,
            cancel_token: None,
            draining_after_cancel: false,
            request_start: None,
            first_token_latency_recorded: false,
            interaction_reply_tx: None,
            pending_user_messages: Arc::new(Mutex::new(VecDeque::new())),
            remote: None,
            remote_session_open: false,
            pending_hook_actions: Vec::new(),
            client_id,
            client_hostname: super::runtime_request::hostname(),
            http_client,
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
        self.cancel_token = None;
        self.draining_after_cancel = false;
        self.request_start = None;
        self.first_token_latency_recorded = false;
        self.interaction_reply_tx = None;
        if let Ok(mut pending) = self.pending_user_messages.lock() {
            pending.clear();
        }
        self.remote_session_open = false;
    }

    /// Mark the remote session as no longer open from this TUI's perspective
    /// (the daemon considers another client the holder). Used by the
    /// heartbeat tick on `TakenOver` so subsequent `disconnect_remote` /
    /// `close_sessions` calls skip the now-meaningless detach RPC.
    pub(crate) fn mark_remote_session_taken_over(&mut self) {
        self.remote_session_open = false;
    }

    pub fn needs_active_refresh(&self) -> bool {
        self.stream_rx.is_some()
            || !self.stream_reveal_buffer.is_empty()
            || self.pending_stream_done.is_some()
    }

    /// Whether a turn's update stream has not fully settled yet: updates
    /// are still flowing (normal streaming or the post-Esc drain), or a
    /// terminal `Done` is pending reveal. Callers that read the backend
    /// session store (manual `/save`, interrupt auto-save) must wait for
    /// this to become `false` first — see
    /// [`GatewayRuntime::settle_in_flight_turn`].
    pub fn turn_stream_in_flight(&self) -> bool {
        self.stream_rx.is_some() || self.pending_stream_done.is_some()
    }

    /// Wait for a turn whose stream is still in flight — typically a turn
    /// cancelled with Esc, whose partial loop state the backend persists
    /// only after the in-flight LLM call returns — to settle. Pumps
    /// `poll_stream_updates` (normally driven by the App event loop) until
    /// the terminal `Done`/`Err` update is consumed, bounded by
    /// [`TURN_SETTLE_TIMEOUT`]. On timeout, the caller proceeds with a
    /// store that may miss the last turn (logged here); this is no worse
    /// than the pre-fix behaviour.
    pub async fn settle_in_flight_turn(&mut self, state: &mut crate::app_state::AppState) {
        if !self.turn_stream_in_flight() && !state.chat_state.is_loading {
            return;
        }
        let deadline = Instant::now() + TURN_SETTLE_TIMEOUT;
        loop {
            self.poll_stream_updates(state);
            if !self.turn_stream_in_flight() && !state.chat_state.is_loading {
                return;
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    timeout = ?TURN_SETTLE_TIMEOUT,
                    "in-flight turn did not settle in time; proceeding with a session store that may miss the last turn"
                );
                return;
            }
            tokio::time::sleep(TURN_SETTLE_POLL_INTERVAL).await;
        }
    }

    /// Load the session record from the backend store and refresh the
    /// TUI's `session_messages` cache from the persisted
    /// `loop_state.messages`. Returns the loaded record so snapshot
    /// writers (`/save`, interrupt auto-save) embed it — `None` when the
    /// store holds no record for the session.
    pub(crate) async fn sync_session_messages_from_store(
        &self,
        state: &mut crate::app_state::AppState,
    ) -> Option<crate::gateway::SessionRecord> {
        let record = self.session_snapshot(&state.session_id).await;
        if let Some(loop_state) = record
            .as_ref()
            .and_then(|record| record.loop_state.as_ref())
        {
            if !loop_state.messages.is_empty() && state.session_messages != loop_state.messages {
                state.session_messages = loop_state.messages.clone();
            }
        }
        record
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

    /// Detaches / closes sessions before exit.
    ///
    /// **Remote mode**: calls `/api/v1/runtimes/detach` so the daemon
    /// releases this TUI's attach lease without destroying the session or
    /// sandbox — the session stays warm for another TUI to pick up via
    /// `/sessions`. Final destruction is left to the daemon's GC reaper
    /// (force-closes sessions with no live lease for ~2 hours) or an explicit
    /// `/remote close`.
    ///
    /// **Local mode**: `shutdown_all()` cleans up backends (no-op for local
    /// backend; delete API call for E2B/Conch).
    pub async fn close_sessions(&mut self, session_id: &str) {
        self.session_gateway.close_all_sessions().await;

        if self.remote.is_some() && self.remote_session_open {
            tracing::info!(session_id = %session_id, "Detaching remote session on daemon");
            self.detach_remote_session_bounded(session_id, "close_sessions")
                .await;
        } else {
            tracing::info!("Shutting down local backends");
            if let Err(error) = self.session_gateway.backend_manager.shutdown_all().await {
                tracing::warn!(error = %error, "failed to shutdown TUI backend manager");
            }
        }
    }

    /// Releases the backend for a specific session (used by /new command).
    /// - For local backend: no-op (just updates state, no sandbox to delete)
    /// - For E2B/Conch backend: calls delete API if no other sessions share it
    pub async fn release_session_backend(&self, session_id: &str) -> Result<(), String> {
        self.session_gateway
            .backend_manager
            .release_session(session_id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Returns whether remote mode is active
    pub fn is_remote_mode(&self) -> bool {
        self.remote.is_some() && self.remote_session_open
    }

    /// Drain and return any hook actions collected from the SSE stream.
    /// Called by the App's event loop after `poll_stream_updates`. The App
    /// executes each action (switch session, set title) asynchronously.
    pub fn take_pending_hook_actions(&mut self) -> Vec<xiaoo_api::chat::HookAction> {
        std::mem::take(&mut self.pending_hook_actions)
    }
}
