use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{Event, EventStream, MouseEventKind};
use crossterm::execute;
use futures_util::{FutureExt, StreamExt};
use ratatui::Terminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::app_state::AppState;
use crate::config::Config;
use crate::gateway_runtime::GatewayRuntime;

pub struct App {
    pub(crate) state: AppState,
    pub(crate) gateway: GatewayRuntime,
    pending_local_model_fetch: Option<tokio::sync::oneshot::Receiver<Vec<crate::chat::ModelInfo>>>,
}

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

impl App {
    pub fn new_with_config(
        config: &Config,
        config_path: PathBuf,
        workspace: PathBuf,
    ) -> Result<Self, anyhow::Error> {
        let state = AppState::new_with_config(config, config_path, workspace)?;
        let gateway = GatewayRuntime::new(state.client_id.clone());
        Ok(Self {
            state,
            gateway,
            pending_local_model_fetch: None,
        })
    }

    pub async fn run(
        &mut self,
        terminal: &mut Terminal<impl ratatui::backend::Backend>,
    ) -> Result<()> {
        #[cfg(debug_assertions)]
        eprintln!("PERF_PROBES_ACTIVE");
        let mut event_stream = EventStream::new();
        let mut pending_event: Option<Event> = None;
        let _ = execute!(io::stdout(), SetCursorStyle::BlinkingBar);
        set_cursor_color(self.state.theme.border_active);
        let mut cursor_visible = true;
        let mut last_cursor_blink_toggle = Instant::now();
        let mut needs_redraw = true;
        // Heartbeat interval for renewing the daemon's attach lease.
        let mut heartbeat_interval =
            tokio::time::interval(crate::gateway_api::http_timeouts::HEARTBEAT_INTERVAL);
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Skip the immediate first tick so we don't fire before any session
        // is open.
        heartbeat_interval.tick().await;

        // Periodic full redraw while idle/ASK: refreshes the header clock
        // (which only updates on `draw`) and recovers from external screen
        // corruption (terminal wake/scrollback clear/reattach) that ratatui's
        // diff optimization would otherwise miss — the previous buffer still
        // matches the last frame, so an unchanged state produces an empty
        // diff and the actual (cleared) screen is never rewritten. Skipped
        // while loading: the 16ms tick already redraws constantly and any
        // streaming state change yields a non-empty diff that recovers the
        // screen naturally. `swap_buffers()` resets the back buffer (without
        // emitting `\x1b[2J`, so no visible clear/flicker) so the next
        // `draw()` writes the full frame instead of a no-op diff.
        let mut force_redraw_interval = tokio::time::interval(Duration::from_secs(1));
        force_redraw_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        force_redraw_interval.tick().await;

        #[cfg(unix)]
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        #[cfg(unix)]
        if sigterm.is_none() {
            tracing::warn!(
                "Failed to register SIGTERM handler, graceful shutdown on SIGTERM unavailable"
            );
        }

        loop {
            #[cfg(debug_assertions)]
            let _tick_start = std::time::Instant::now();
            if needs_redraw {
                #[cfg(debug_assertions)]
                let _draw_start = std::time::Instant::now();
                terminal.draw(|frame| self.ui(frame))?;
                needs_redraw = false;
                #[cfg(debug_assertions)]
                {
                    let draw_elapsed = _draw_start.elapsed();
                    if draw_elapsed > std::time::Duration::from_micros(500) {
                        eprintln!("PERF terminal_draw: {}µs", draw_elapsed.as_micros());
                    }
                }
            }

            #[cfg(debug_assertions)]
            {
                let tick_elapsed = _tick_start.elapsed();
                if tick_elapsed > std::time::Duration::from_millis(15) && self.state.chat_state.is_loading {
                    eprintln!("PERF event_loop_tick: {}µs active_refresh=true", tick_elapsed.as_micros());
                }
            }
            if last_cursor_blink_toggle.elapsed() >= CURSOR_BLINK_INTERVAL {
                cursor_visible = !cursor_visible;
                last_cursor_blink_toggle = Instant::now();
                if cursor_visible {
                    terminal.show_cursor()?;
                } else {
                    terminal.hide_cursor()?;
                }
            }

            let active_refresh =
                self.state.chat_state.is_loading || self.gateway.needs_active_refresh();
            let tick_duration = if active_refresh {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(250)
            };

            let mut handled_event = None;
            if let Some(event) = pending_event.take() {
                self.handle_event(event.clone()).await?;
                needs_redraw = true;
                handled_event = Some(event);
            } else {
                #[cfg(unix)]
                {
                    tokio::select! {
                        _ = sleep(tick_duration) => {
                            if self.state.chat_state.is_loading {
                                self.state.loading_tick = (self.state.loading_tick + 1) % 12;
                                needs_redraw = true;
                            }
                        }
                        maybe_event = event_stream.next().fuse() => {
                            if let Some(Ok(event)) = maybe_event {
                                self.handle_event(event.clone()).await?;
                                needs_redraw = true;
                                handled_event = Some(event);
                            }
                        }
                        models = wait_for_local_models(&mut self.pending_local_model_fetch) => {
                            self.pending_local_model_fetch = None;
                            if let Some(models) = models {
                                if let Some(dialog) = self.state.provider_dialog.as_mut() {
                                    dialog.apply_fetched_local_models(models);
                                }
                            }
                            needs_redraw = true;
                        }
                        _ = heartbeat_interval.tick() => {
                            needs_redraw = self.run_heartbeat_tick().await || needs_redraw;
                        }
                        _ = force_redraw_interval.tick() => {
                            if !self.state.chat_state.is_loading {
                                terminal.swap_buffers();
                                needs_redraw = true;
                            }
                        }
                        _ = tokio::signal::ctrl_c() => {
                            tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
                            self.state.should_quit = true;
                            self.state.quit_via_interrupt = true;
                        }
                        _ = async {
                            match &mut sigterm {
                                Some(s) => s.recv().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            tracing::info!("Received SIGTERM, initiating graceful shutdown");
                            self.state.should_quit = true;
                            self.state.quit_via_interrupt = true;
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    tokio::select! {
                        _ = sleep(tick_duration) => {
                            if self.state.chat_state.is_loading {
                                self.state.loading_tick = (self.state.loading_tick + 1) % 12;
                                needs_redraw = true;
                            }
                        }
                        maybe_event = event_stream.next().fuse() => {
                            if let Some(Ok(event)) = maybe_event {
                                self.handle_event(event.clone()).await?;
                                needs_redraw = true;
                                handled_event = Some(event);
                            }
                        }
                        models = wait_for_local_models(&mut self.pending_local_model_fetch) => {
                            self.pending_local_model_fetch = None;
                            if let Some(models) = models {
                                if let Some(dialog) = self.state.provider_dialog.as_mut() {
                                    dialog.apply_fetched_local_models(models);
                                }
                            }
                            needs_redraw = true;
                        }
                        _ = heartbeat_interval.tick() => {
                            needs_redraw = self.run_heartbeat_tick().await || needs_redraw;
                        }
                        _ = force_redraw_interval.tick() => {
                            if !self.state.chat_state.is_loading {
                                terminal.swap_buffers();
                                needs_redraw = true;
                            }
                        }
                        _ = tokio::signal::ctrl_c() => {
                            tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
                            self.state.should_quit = true;
                            self.state.quit_via_interrupt = true;
                        }
                    }
                }
            }

            if let Some(event) = handled_event.as_ref() {
                discard_redundant_boundary_scrolls(
                    event,
                    &self.state,
                    &mut event_stream,
                    &mut pending_event,
                );
            }

            needs_redraw |= self.gateway.poll_stream_updates(&mut self.state);
            if self.state.should_quit {
                break;
            }

            // Drain and execute any hook actions received from the daemon
            // (create/switch session, set title).
            //
            // Remote (daemon) mode: actions arrive via the SSE `Done`
            // event's `actions` field after the daemon has already run
            // daemon-side effects (open_session) via
            // `DaemonHookActionSink`; the TUI only needs to switch UI
            // focus and sync its local registry.
            //
            // Local (non-daemon) mode: actions arrive via
            // `SessionTurnUpdate::HookActions` (no SSE, no daemon).
            // `execute_hook_action` → `switch_to_remote_session` checks
            // `remote_base_url()`, finds None, and drops each action with
            // a `tracing::warn!`. This is intentional: create_session /
            // switch_session are not supported in local mode.
            let pending_actions = self.gateway.take_pending_hook_actions();
            if !pending_actions.is_empty() {
                for action in pending_actions {
                    self.execute_hook_action(action).await;
                }
                needs_redraw = true;
            }

            if !self.state.chat_state.is_loading && self.state.chat_state.has_pending_turns() {
                match self.gateway.start_next_queued_turn(&mut self.state).await {
                    Ok(started) => {
                        needs_redraw |= started;
                    }
                    Err(error) => {
                        self.state
                            .chat_state
                            .messages
                            .push(crate::chat::Message::error(error));
                        self.state.chat_state.stick_to_bottom = true;
                        needs_redraw = true;
                    }
                }
            }

            #[cfg(debug_assertions)]
            {
                let tick_elapsed = _tick_start.elapsed();
                if tick_elapsed > std::time::Duration::from_millis(15) && self.state.chat_state.is_loading {
                    eprintln!("PERF event_loop_tick: {}µs active_refresh=true", tick_elapsed.as_micros());
                }
            }
        }
        // If the user interrupted the runtime (Ctrl+C / SIGINT / SIGTERM),
        // persist the session to its rolling automatic slot. Manual
        // checkpoints are never written by this shutdown path.
        if self.state.quit_via_interrupt {
            let record = self.gateway.session_snapshot(&self.state.session_id).await;
            match crate::session_snapshot_service::autosave_on_interrupt(&self.state, record) {
                Ok(Some(path)) => {
                    tracing::info!("Auto-saved session on interrupt to {}", path.display());
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!("Failed to auto-save session on interrupt: {error:#}");
                }
            }
        }

        self.gateway.close_sessions(&self.state.session_id).await;
        reset_cursor_color();
        terminal.show_cursor()?;
        Ok(())
    }

    pub fn start_local_model_fetch(&mut self, api_base: String) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_local_model_fetch = Some(rx);
        tokio::spawn(async move {
            let models = fetch_models_from_local_api(&api_base).await;
            let _ = tx.send(models);
        });
    }

    /// Periodic attach-lease heartbeat. Runs every 15 s while the TUI is in
    /// remote mode. On `TakenOver { stale: Some(true) }` it first attempts a
    /// reclaim via `open_remote_session_with_record` (the recorded holder is
    /// itself stale, so `open_session`'s `acquire` will take it over for us,
    /// avoiding a needless takeover notice). On reclaim failure or a live
    /// holder it flips `session_taken_over = true` and pushes a notice so
    /// `handle_event` can short-circuit submissions instead of letting them
    /// fail at the daemon. `Network` errors are just logged — the next tick
    /// retries; a long outage eventually resolves as `TakenOver` or recovery.
    ///
    /// Returns `true` iff this tick changed visible state (so the caller can
    /// skip a needless redraw on `Network` errors / `Ok(())`).
    async fn run_heartbeat_tick(&mut self) -> bool {
        if !self.gateway.is_remote_mode() || self.state.session_taken_over {
            return false;
        }
        let session_id = self.state.session_id.clone();
        match self.gateway.heartbeat_remote_session(&session_id).await {
            Ok(()) => false,
            Err(crate::gateway_api::remote::HeartbeatError::TakenOver { detail, stale }) => {
                // Stale lease: the recorded holder is presumed dead. Attempt
                // a reclaim before flagging a takeover — saves the user a
                // manual `/remote` or `/sessions` round-trip. Only `Some(true)`
                // reclaims; `None` (parse failure) and `Some(false)` (live
                // holder) fall through to the taken-over notice so a parse
                // regression or genuinely-live holder never widens the bypass.
                if stale == Some(true) {
                    tracing::info!(
                        session_id = %session_id,
                        "heartbeat rejected with stale lease; attempting to reclaim via open"
                    );
                    if self
                        .gateway
                        .open_remote_session_with_record(&mut self.state)
                        .await
                        .is_ok()
                    {
                        tracing::info!(
                            session_id = %session_id,
                            "reclaimed stale lease via open; resuming normal operation"
                        );
                        return true;
                    }
                    // Reclaim failed (another client beat us, or network).
                }
                tracing::warn!(
                    session_id = %session_id,
                    detail = %detail,
                    "session taken over by another xiaoo process; further submissions will be rejected"
                );
                self.state.session_taken_over = true;
                // Sync `remote_session_open` so `disconnect_remote` /
                // `close_sessions` skip the now-meaningless detach (the lease
                // is held by another client; our detach would be a no-op).
                self.gateway.mark_remote_session_taken_over();
                self.state
                    .chat_state
                    .messages
                    .push(crate::chat::Message::system(
                        crate::gateway_api::runtime_request::session_taken_over_notice_glyph(),
                    ));
                self.state.chat_state.stick_to_bottom = true;
                true
            }
            Err(crate::gateway_api::remote::HeartbeatError::Network(error)) => {
                tracing::warn!(error = %error, "session heartbeat failed; will retry next tick");
                false
            }
        }
    }

    /// Execute a single hook action received after a turn terminates.
    ///
    /// Remote (daemon) mode: the daemon has already executed daemon-side
    /// effects (open_session for create/switch) via `DaemonHookActionSink`
    /// before forwarding; the TUI only needs to switch UI focus.
    ///
    /// Local (non-daemon) mode: there is no daemon, so no `open_session`
    /// has run. `switch_to_remote_session` checks `remote_base_url()`,
    /// finds None, and drops the action with a `tracing::warn!`. This is
    /// intentional — create_session / switch_session are not supported in
    /// local mode. See [`switch_to_remote_session`].
    async fn execute_hook_action(&mut self, action: agent_types::hook::HookAction) {
        match action {
            agent_types::hook::HookAction::CreateSession { session_id } => {
                tracing::info!(
                    session_id = %session_id,
                    "TUI: hook action create_session — switching focus"
                );
                self.switch_to_remote_session(session_id).await;
            }
            agent_types::hook::HookAction::SwitchSession { session_id } => {
                // Skip the switch (and the transcript reload + "Switched to
                // session" system message it inserts) when the TUI is already
                // focused on the target. This commonly happens when a plugin
                // emits `[create_session X, switch_session X, send_prompt X]`
                // in one batch: create already moved focus to X, so the
                // following switch_session would no-op on the daemon
                // (idempotent `open_session`) but still churn the TUI side.
                if self.state.session_id == session_id {
                    tracing::info!(
                        session_id = %session_id,
                        "TUI: hook action switch_session — already focused, skipping"
                    );
                    return;
                }
                tracing::info!(
                    session_id = %session_id,
                    "TUI: hook action switch_session — switching focus"
                );
                self.switch_to_remote_session(session_id).await;
            }
            agent_types::hook::HookAction::SendPrompt {
                session_id,
                text,
                chain_depth,
            } => {
                // Only takes effect in remote (daemon) mode, matching
                // create_session / switch_session: the action's visible
                // execution (echo + streaming) requires a remote backend the
                // TUI can POST `/runtimes/input` to. In local mode the action
                // is dropped with a warning.
                if self.gateway.remote_base_url().is_none() {
                    tracing::warn!(
                        session_id = %session_id,
                        "send_prompt hook action dropped (local mode does not execute daemon-side actions)"
                    );
                    return;
                }
                if text.trim().is_empty() {
                    tracing::warn!(
                        session_id = %session_id,
                        "send_prompt hook action dropped (empty text)"
                    );
                    return;
                }
                // Switch focus only if not already on the target session;
                // skipping avoids a transcript reload and a redundant
                // "Switched to session" system message. The daemon has
                // already called `open_session` for the target (idempotent)
                // in `DaemonHookActionSink::execute_on_daemon`.
                if self.state.session_id != session_id {
                    tracing::info!(
                        session_id = %session_id,
                        chain_depth,
                        "TUI: hook action send_prompt — switching focus"
                    );
                    self.switch_to_remote_session(session_id).await;
                }
                // After a switch, `apply_remote_session_switch` resets
                // `is_loading` (via `reset_for_new_session`), so the start_turn
                // branch below runs. If the switch was skipped and a turn is
                // still running (e.g. turn A's reveal buffer hasn't fully
                // drained when Done arrived), enqueue so `start_next_queued_turn`
                // drains it once idle — same behavior as a user typing while
                // a turn is running.
                if self.state.chat_state.is_loading {
                    tracing::info!(
                        chain_depth,
                        "TUI: hook action send_prompt — turn in progress, enqueueing"
                    );
                    self.state
                        .chat_state
                        .enqueue_pending_turn(text, None, chain_depth);
                } else if let Err(error) = self
                    .gateway
                    .start_turn_for_hook_prompt(&mut self.state, text, chain_depth)
                    .await
                {
                    tracing::warn!(
                        error = %error,
                        "TUI: hook action send_prompt — start_turn failed"
                    );
                }
            }
        }
    }

    /// Switch the TUI focus to a remote session. The daemon has already
    /// called `/runtimes/open` for the target session; the TUI resets its
    /// state, sets the new session_id, and configures remote mode so the
    /// next turn uses the existing session on the daemon.
    ///
    /// Invoked both by hook actions (Create/SwitchSession received from the
    /// daemon after a turn ends) and by the TUI's `/sessions` built-in
    /// command for manual switching. In local (non-daemon) mode there is no
    /// remote backend to switch to, so the action is dropped with a
    /// `tracing::warn` — daemon-side effects (open_session) cannot be
    /// reproduced locally by this path.
    pub(crate) async fn switch_to_remote_session(&mut self, session_id: String) {
        let Some(base_url) = self.gateway.remote_base_url().map(str::to_string) else {
            tracing::warn!(
                session_id = %session_id,
                "switch_to_remote_session called without remote backend; \
                 hook action dropped (local mode does not execute daemon-side actions)"
            );
            return;
        };
        let token_env = self
            .state
            .agent_config
            .tui
            .remote
            .as_ref()
            .and_then(|remote| remote.bearer_token_env.clone());

        self.apply_remote_session_switch(
            session_id.clone(),
            base_url,
            token_env,
            format!("Switched to session: {session_id}"),
        )
        .await;
    }

    /// Shared body of [`switch_to_remote_session`] and
    /// [`activate_remote_session`]: reset local state for `session_id`,
    /// configure the remote backend, restore the prior transcript from the
    /// daemon, persist the remote-session record, and push `system_message`.
    ///
    /// The daemon's `open_session` call is wrapped in a `tokio::time::timeout`
    /// so an unreachable daemon cannot block the TUI event loop
    /// indefinitely — on timeout we log and continue with an empty
    /// transcript rather than hanging the UI.
    pub(crate) async fn apply_remote_session_switch(
        &mut self,
        session_id: String,
        base_url: String,
        bearer_token_env: Option<String>,
        system_message: String,
    ) {
        self.gateway.reset_for_new_session(&mut self.state);
        self.state.reset_for_new_session();
        self.state.session_id = session_id.clone();
        self.gateway
            .configure_remote(&mut self.state, base_url.clone(), bearer_token_env.clone());

        // Restore conversation context from the daemon. `open_session` is
        // idempotent on the daemon side; calling it here both ensures the
        // session is opened on the daemon (so `remote_session_open` becomes
        // true and the next turn skips the open call) and gives us back
        // the stored `SessionRecord` so we can repopulate local state.
        // Bound the wait so a dead/unreachable daemon does not freeze the
        // TUI event loop.
        let restored_messages = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.gateway
                .open_remote_session_with_record(&mut self.state),
        )
        .await
        {
            Ok(Ok(record)) => {
                let loop_messages = record.loop_state.map(|ls| ls.messages).unwrap_or_default();
                let display_messages =
                    crate::chat::messages_from_chat_messages(loop_messages.clone());
                self.state.session_messages = loop_messages;
                display_messages
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "remote session switch: failed to fetch session record from daemon; \
                     transcript will be empty until the next turn"
                );
                Vec::new()
            }
            Err(_) => {
                tracing::warn!(
                    session_id = %session_id,
                    "remote session switch: open_session timed out after 10 seconds; \
                     transcript will be empty until the next turn"
                );
                Vec::new()
            }
        };

        if !restored_messages.is_empty() {
            self.state.chat_state.messages.extend(restored_messages);
            self.state.chat_state.stick_to_bottom = true;
        }

        let _ = crate::remote_sessions_service::record_remote_session(
            &session_id,
            &base_url,
            bearer_token_env,
            None,
        );

        self.state
            .chat_state
            .messages
            .push(crate::chat::Message::system(system_message));
        self.state.chat_state.stick_to_bottom = true;
    }
}

async fn wait_for_local_models(
    rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<crate::chat::ModelInfo>>>,
) -> Option<Vec<crate::chat::ModelInfo>> {
    match rx {
        Some(inner) => match inner.await {
            Ok(models) => models.into(),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

async fn fetch_models_from_local_api(api_base: &str) -> Vec<crate::chat::ModelInfo> {
    let url = format!("{}/models", api_base.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let response = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let body: serde_json::Value = match response.json().await {
        Ok(b) => b,
        Err(_) => return vec![],
    };
    let models = body["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|model| {
                    model["id"].as_str().map(|id| crate::chat::ModelInfo {
                        id: id.to_string(),
                        name: id.to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if models.is_empty() {
        return vec![];
    }
    models
}

fn discard_redundant_boundary_scrolls(
    handled_event: &Event,
    state: &AppState,
    event_stream: &mut EventStream,
    pending_event: &mut Option<Event>,
) {
    let boundary_kind = match handled_event {
        Event::Mouse(mouse)
            if mouse.kind == MouseEventKind::ScrollDown
                && state.active_transcript_scroll_offset()
                    >= state.active_transcript_max_scroll_offset() =>
        {
            Some(MouseEventKind::ScrollDown)
        }
        Event::Mouse(mouse)
            if mouse.kind == MouseEventKind::ScrollUp
                && state.active_transcript_scroll_offset() == 0 =>
        {
            Some(MouseEventKind::ScrollUp)
        }
        _ => None,
    };

    let Some(boundary_kind) = boundary_kind else {
        return;
    };
    let opposite_kind = match boundary_kind {
        MouseEventKind::ScrollDown => MouseEventKind::ScrollUp,
        MouseEventKind::ScrollUp => MouseEventKind::ScrollDown,
        _ => return,
    };

    for _ in 0..128 {
        let Some(ready) = event_stream.next().now_or_never() else {
            break;
        };
        let Some(Ok(event)) = ready else {
            break;
        };

        match &event {
            Event::Mouse(mouse) if mouse.kind == boundary_kind => {
                continue;
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved => {
                continue;
            }
            Event::Mouse(mouse) if mouse.kind == opposite_kind => {
                *pending_event = Some(event);
                return;
            }
            _ => {
                *pending_event = Some(event);
                return;
            }
        }
    }
}

fn set_cursor_color(color: ratatui::style::Color) {
    let Some(value) = color_to_ansi(color) else {
        return;
    };
    let _ = io::stdout().write_all(format!("\x1b]12;{value}\x07").as_bytes());
    let _ = io::stdout().flush();
}

fn reset_cursor_color() {
    let _ = io::stdout().write_all(b"\x1b]112\x07");
    let _ = io::stdout().flush();
}

fn color_to_ansi(color: ratatui::style::Color) -> Option<String> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        ratatui::style::Color::Black => Some("black".to_string()),
        ratatui::style::Color::Red => Some("red".to_string()),
        ratatui::style::Color::Green => Some("green".to_string()),
        ratatui::style::Color::Yellow => Some("yellow".to_string()),
        ratatui::style::Color::Blue => Some("blue".to_string()),
        ratatui::style::Color::Magenta => Some("magenta".to_string()),
        ratatui::style::Color::Cyan => Some("cyan".to_string()),
        ratatui::style::Color::Gray => Some("gray".to_string()),
        ratatui::style::Color::DarkGray => Some("darkgray".to_string()),
        ratatui::style::Color::LightRed => Some("lightred".to_string()),
        ratatui::style::Color::LightGreen => Some("lightgreen".to_string()),
        ratatui::style::Color::LightYellow => Some("lightyellow".to_string()),
        ratatui::style::Color::LightBlue => Some("lightblue".to_string()),
        ratatui::style::Color::LightMagenta => Some("lightmagenta".to_string()),
        ratatui::style::Color::LightCyan => Some("lightcyan".to_string()),
        ratatui::style::Color::White => Some("white".to_string()),
        ratatui::style::Color::Indexed(index) => Some(index.to_string()),
        ratatui::style::Color::Reset => None,
    }
}
