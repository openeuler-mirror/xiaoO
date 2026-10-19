use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{Event, EventStream, MouseEventKind};
use crossterm::execute;
use futures_util::{FutureExt, StreamExt};
use ratatui::Terminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::sleep_until;

use crate::app_state::AppState;
use crate::config::Config;
use crate::gateway_runtime::GatewayRuntime;

pub struct App {
    pub(crate) state: AppState,
    pub(crate) gateway: GatewayRuntime,
    /// 官方 model list 拉取结果的回传通道。
    ///
    /// 用 mpsc 而不是 oneshot：拉取按 provider 独立进行，快速上下键切换
    /// provider 时允许多个请求在飞（上限见 `MAX_INFLIGHT_MODEL_FETCHES`），
    /// 每个任务都必须能把结果回传——否则被顶掉的那个 provider 会卡在 `Loading`
    /// 直到陈旧兜底窗口（`MODEL_LIST_STALE_AFTER`）到期后的下一次触发，见
    /// `start_model_fetch`。结果按 provider 名应用，与顺序无关。
    pub(crate) model_fetch_tx: tokio::sync::mpsc::UnboundedSender<(
        u64,
        crate::provider_service::ProviderModelFetchOutcome,
    )>,
    pub(crate) model_fetch_rx: tokio::sync::mpsc::UnboundedReceiver<(
        u64,
        crate::provider_service::ProviderModelFetchOutcome,
    )>,
    /// Deadline for discarding stray character events after an ESC key.
    ///
    /// When an ESC byte lands next to a mouse/scroll escape sequence
    /// (`\x1b[<b;x;yM`), crossterm's parser can swallow the sequence head
    /// and deliver the leftover `[<b;x;yM` as plain `KeyCode::Char` events.
    /// Those always arrive back-to-back with the ESC event (same read
    /// batch, <1ms apart), so any `Char` received within this window is a
    /// sequence remnant, not real typing — discard it. Human key presses
    /// are >50ms apart, so a 5ms window never eats legitimate input.
    pub(crate) esc_discard_until: Option<Instant>,
    /// True while a mouse drag that started inside the input box is in
    /// progress. The input-box `Up` handler only auto-copies when this is
    /// set, so a left-button release elsewhere (tool toggle, scrollbar,
    /// transcript area) can never copy the input box's lingering selection
    /// (e.g. one created with Ctrl+A).
    pub(crate) input_drag_active: bool,
    /// Wall-clock origin for the loading spinner (see `loading_animation`).
    /// The spinner frame is `elapsed_ms / 16 % FRAMES`, decoupling the
    /// animation cadence from the event-loop cycle time.
    pub(crate) animation_origin: Instant,
}

impl App {
    pub fn new_with_config(
        config: &Config,
        config_path: PathBuf,
        workspace: PathBuf,
    ) -> Result<Self, anyhow::Error> {
        let state = AppState::new_with_config(config, config_path, workspace)?;
        let gateway = GatewayRuntime::new(state.client_id.clone());
        let (model_fetch_tx, model_fetch_rx) = tokio::sync::mpsc::unbounded_channel();
        Ok(Self {
            state,
            gateway,
            model_fetch_tx,
            model_fetch_rx,
            esc_discard_until: None,
            input_drag_active: false,
            animation_origin: Instant::now(),
        })
    }

    pub async fn run(
        &mut self,
        terminal: &mut Terminal<impl ratatui::backend::Backend>,
    ) -> Result<()> {
        #[cfg(debug_assertions)]
        tracing::debug!("PERF_PROBES_ACTIVE");
        let mut event_stream = EventStream::new();
        let mut pending_event: Option<Event> = None;
        let _ = execute!(io::stdout(), SetCursorStyle::SteadyBar);
        set_cursor_color(self.state.theme.border_active);
        let mut needs_redraw = true;
        // Absolute deadline of the next animation frame while streaming.
        // `Delay`-style: each tick advances by a fixed 16ms from the previous
        // deadline (in the select! sleep branch — never per-iteration), so the
        // draw time is compensated and the cycle stays at ~16ms (60fps) as
        // long as draw < 16ms, while fast event-driven iterations cannot push
        // the deadline into the future. Started at `now`: the first active
        // iteration's `next_wake` equals it, so the deadline falls in the past
        // once the initial draw has run and is reset to `now + period`.
        let animation_period = Duration::from_millis(16);
        let mut next_animation_frame = tokio::time::Instant::now();
        // Heartbeat interval for renewing the daemon's attach lease.
        let mut heartbeat_interval =
            tokio::time::interval(crate::gateway_api::http_timeouts::HEARTBEAT_INTERVAL);
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Skip the immediate first tick so we don't fire before any session
        // is open.
        heartbeat_interval.tick().await;

        // Idle/ASK refresh, split by purpose:
        //
        // - 1s diff redraw: keeps the header clock (`%H:%M:%S`) and the
        //   status light ticking. A normal `draw()` suffices — ratatui's
        //   diff emits only the changed cells, a few bytes per tick.
        // - 30s full repaint (`swap_buffers()`): recovers from external
        //   screen corruption (terminal wake/scrollback clear/reattach)
        //   that ratatui's diff optimization would otherwise miss — the
        //   previous buffer still matches the last frame, so an unchanged
        //   state produces an empty diff and the actual (cleared) screen
        //   is never rewritten. `swap_buffers()` resets the back buffer
        //   (without emitting `\x1b[2J`, so no visible clear/flicker) so
        //   the next `draw()` writes the full frame instead of a no-op
        //   diff. Full rewrites are the largest output the app produces
        //   (one MoveTo per screen row, re-sent even when nothing
        //   changed), so they are kept rare — FrameBackend ships each as
        //   a single write, but on slow links (SSH/mux) the terminal may
        //   still paint between network chunks and briefly show the
        //   hardware cursor at those intermediate positions.
        //
        // Both are skipped while loading: the 16ms tick already redraws
        // constantly and any streaming state change yields a non-empty
        // diff that recovers the screen naturally.
        let mut clock_refresh_interval = tokio::time::interval(Duration::from_secs(1));
        clock_refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        clock_refresh_interval.tick().await;
        let mut corruption_recovery_interval = tokio::time::interval(Duration::from_secs(30));
        corruption_recovery_interval
            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        corruption_recovery_interval.tick().await;

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
                        tracing::debug!("PERF terminal_draw: {}µs", draw_elapsed.as_micros());
                    }
                }
            }

            #[cfg(debug_assertions)]
            {
                let tick_elapsed = _tick_start.elapsed();
                if tick_elapsed > std::time::Duration::from_millis(15)
                    && self.state.chat_state.is_loading
                {
                    tracing::debug!(
                        "PERF event_loop_tick: {}µs active_refresh=true",
                        tick_elapsed.as_micros()
                    );
                }
            }

            let active_refresh =
                self.state.chat_state.is_loading || self.gateway.needs_active_refresh();
            // While streaming, wake at the absolute animation deadline (Delay-
            // style: compensated for draw time → ~16ms cycle = 60fps). While
            // idle, a 250ms relative sleep is enough — the spinner doesn't
            // run and `clock_refresh_interval` (1s) handles idle refresh.
            //
            // The deadline is ADVANCED only when the sleep actually fires
            // (see the select! sleep branch), never here: advancing per loop
            // iteration would let a burst of fast event-driven iterations
            // (scroll wheel, key auto-repeat) push the deadline 16ms into the
            // future each time, stalling the loop until it caught up.
            let next_wake = if active_refresh {
                let now = tokio::time::Instant::now();
                // Fallen behind (idle→active switch, or draw > period):
                // restart the deadline from now instead of firing a catch-up
                // burst.
                if next_animation_frame < now {
                    next_animation_frame = now + animation_period;
                }
                next_animation_frame
            } else {
                tokio::time::Instant::now() + Duration::from_millis(250)
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
                        _ = sleep_until(next_wake) => {
                            // Advance the animation deadline only when this
                            // sleep fires (Delay semantics) — see the
                            // `next_wake` comment above. Spinner frames are
                            // wall-clock derived; only `is_loading` needs the
                            // forced redraw (the reveal drain and stream
                            // updates drive their own redraws via
                            // `poll_stream_updates`).
                            if active_refresh {
                                next_animation_frame += animation_period;
                                if self.state.chat_state.is_loading {
                                    needs_redraw = true;
                                }
                            }
                        }
                        maybe_event = event_stream.next().fuse() => {
                            if let Some(Ok(event)) = maybe_event {
                                self.handle_event(event.clone()).await?;
                                needs_redraw = true;
                                handled_event = Some(event);
                            }
                        }
                        outcome = self.model_fetch_rx.recv() => {
                            if let Some((session, outcome)) = outcome {
                                self.apply_model_fetch_outcome(session, outcome);
                            }
                            needs_redraw = true;
                        }
                        _ = heartbeat_interval.tick() => {
                            needs_redraw = self.run_heartbeat_tick().await || needs_redraw;
                        }
                        _ = clock_refresh_interval.tick() => {
                            // Light diff redraw: only the clock/status cells
                            // are re-emitted (see the interval setup above).
                            if !self.state.chat_state.is_loading {
                                needs_redraw = true;
                            }
                        }
                        _ = corruption_recovery_interval.tick() => {
                            // Full-frame rewrite to recover from external
                            // corruption; see the interval setup above.
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
                        _ = sleep_until(next_wake) => {
                            // Advance the animation deadline only when this
                            // sleep fires (Delay semantics) — see the
                            // `next_wake` comment above. Spinner frames are
                            // wall-clock derived; only `is_loading` needs the
                            // forced redraw (the reveal drain and stream
                            // updates drive their own redraws via
                            // `poll_stream_updates`).
                            if active_refresh {
                                next_animation_frame += animation_period;
                                if self.state.chat_state.is_loading {
                                    needs_redraw = true;
                                }
                            }
                        }
                        maybe_event = event_stream.next().fuse() => {
                            if let Some(Ok(event)) = maybe_event {
                                self.handle_event(event.clone()).await?;
                                needs_redraw = true;
                                handled_event = Some(event);
                            }
                        }
                        outcome = self.model_fetch_rx.recv() => {
                            if let Some((session, outcome)) = outcome {
                                self.apply_model_fetch_outcome(session, outcome);
                            }
                            needs_redraw = true;
                        }
                        _ = heartbeat_interval.tick() => {
                            needs_redraw = self.run_heartbeat_tick().await || needs_redraw;
                        }
                        _ = clock_refresh_interval.tick() => {
                            // Light diff redraw: only the clock/status cells
                            // are re-emitted (see the interval setup above).
                            if !self.state.chat_state.is_loading {
                                needs_redraw = true;
                            }
                        }
                        _ = corruption_recovery_interval.tick() => {
                            // Full-frame rewrite to recover from external
                            // corruption; see the interval setup above.
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
                coalesce_wheel_scroll_burst(self, event, &mut pending_event).await?;
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
                if tick_elapsed > std::time::Duration::from_millis(15)
                    && self.state.chat_state.is_loading
                {
                    tracing::debug!(
                        "PERF event_loop_tick: {}µs active_refresh=true",
                        tick_elapsed.as_micros()
                    );
                }
            }
        }
        // If the user interrupted the runtime (Ctrl+C / SIGINT / SIGTERM),
        // persist the session to its rolling automatic slot. Manual
        // checkpoints are never written by this shutdown path.
        if self.state.quit_via_interrupt {
            // A turn interrupted by Ctrl+C does not stop by itself: the
            // backend persists the partial loop state only after the
            // in-flight LLM call returns. Cancel any in-flight turn, wait
            // for it to settle (bounded by a timeout), and refresh the
            // TUI's `session_messages` cache from the persisted
            // `loop_state.messages` before reading the store, so the
            // auto-save keeps the conversation context instead of the
            // pre-turn state.
            if self.state.chat_state.is_loading || self.gateway.turn_stream_in_flight() {
                self.gateway.cancel_streaming(&mut self.state);
            }
            self.gateway.settle_in_flight_turn(&mut self.state).await;
            let record = self
                .gateway
                .sync_session_messages_from_store(&mut self.state)
                .await;
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
        // FrameBackend stages cursor commands instead of flushing each one
        // (see app/frame_backend.rs); push the trailing Show out before
        // main.rs' teardown sequences hit stdout.
        terminal.backend_mut().flush()?;
        Ok(())
    }

    /// 发起一次官方 model list 拉取（`provider` 为弹窗里的 provider 名）。
    ///
    /// 守卫两条：目标 provider 已有请求在飞时不重复发起；在飞请求数达到
    /// [`MAX_INFLIGHT_MODEL_FETCHES`] 时跳过（超出的 provider 保持 `Builtin`，
    /// 下次高亮时再拉）。任务只负责回传结果，`apply_model_fetch_outcome`
    /// 负责落地，因此这里可以先同步把状态置为 `Loading` 让 UI 立刻反应。
    pub(crate) fn start_model_fetch(&mut self, provider: String) {
        if provider.trim().is_empty() {
            return;
        }
        // 陈旧的 `Loading`（结果不会再回来）先清掉在飞登记，否则它既拉不动、又永久
        // 占着名额。
        let now = Instant::now();
        if self
            .state
            .chat_state
            .model_list_state(&provider)
            .is_stale_loading(now)
        {
            self.state.finish_model_fetch(&provider);
        }
        if !self.state.begin_model_fetch(&provider) {
            return;
        }
        self.state.chat_state.set_model_list_state(
            &provider,
            crate::chat::ModelListState::Loading { since: now },
        );
        let request =
            crate::provider_service::model_fetch_request(&self.state.agent_config, &provider);
        let session = self.state.model_fetch_session;
        let tx = self.model_fetch_tx.clone();
        tokio::spawn(async move {
            // 内层任务单独 spawn 再 await，是为了拿到 `JoinHandle`：任务 panic 时
            // `handle.await` 返回 `Err`，`join_fetch_outcome` 据此合成失败结果，保证状态
            // 一定能离开 `Loading`。
            let handle = tokio::spawn(crate::provider_service::fetch_provider_models(request));
            let outcome = crate::provider_service::join_fetch_outcome(provider, handle).await;
            // 接收端随 App 一同存活；发送失败只说明 TUI 正在退出。
            let _ = tx.send((session, outcome));
        });
    }

    /// 按需刷新当前高亮 provider 的官方列表。
    ///
    /// `force` 来自 `Ctrl+R`：无视 TTL/失败退避强制重拉（用户刚补完 API key
    /// 就按它，不必等 30s 退避）。非强制路径走 [`ModelListState::needs_refresh`]。
    pub(crate) fn maybe_refresh_selected_provider(&mut self, force: bool) {
        let Some(provider) = self
            .state
            .provider_dialog
            .as_ref()
            .and_then(|dialog| dialog.selected_provider_name().map(str::to_string))
        else {
            return;
        };
        if force
            || self
                .state
                .chat_state
                .model_list_state(&provider)
                .needs_refresh(Instant::now())
        {
            self.start_model_fetch(provider);
        }
    }

    /// 落地一次拉取结果。
    ///
    /// 结果按 provider 名应用，因此弹窗已关闭、或用户已经切到别的 provider
    /// 都不会错位：`ChatState` 始终更新（下次 `/connect` 直接看到官方列表），
    /// 弹窗副本只在它还持有该 provider 时更新，且选中项只在被替换的 provider
    /// 就是当前高亮项时才重算。
    ///
    /// 成功路径把同一份列表写进两处（弹窗 + `available_providers`），失败路径
    /// 一律不碰列表——"成功一次后永久替换内置列表"由此成立。
    pub(crate) fn apply_model_fetch_outcome(
        &mut self,
        session: u64,
        outcome: crate::provider_service::ProviderModelFetchOutcome,
    ) {
        use crate::provider_service::ModelFetchError;
        let provider = outcome.provider;
        // 无论结果是否被采纳，名额都要释放（`join_fetch_outcome` 保证任务一定回传）。
        self.state.finish_model_fetch(&provider);
        if session != self.state.model_fetch_session {
            // 会话已被重建（`/new`、`/load` 与远端会话的连接/切换都会重建）：这是旧会话的结果，丢弃。
            return;
        }
        match outcome.result {
            Ok(models) if !models.is_empty() => {
                // 官方列表不含当前激活模型时补到首位（两家副本用同一份结果）。
                let configured = self
                    .state
                    .agent_config
                    .llm
                    .provider
                    .eq_ignore_ascii_case(&provider)
                    .then(|| self.state.agent_config.llm.model.clone());
                let models =
                    crate::provider_service::pin_configured_model(models, configured.as_deref());
                if let Some(dialog) = self.state.provider_dialog.as_mut() {
                    dialog.replace_models(&provider, models.clone());
                }
                // 目录里已经没有这个 provider 时写不进去（防御分支：会话重建会同时重置
                // 目录与状态，那种情形在世代检查处已被丢弃）：这时记 `Builtin` 而不是
                // `Ready`，否则下次打开弹窗会拿着内置列表显示"官方模型列表"并在 TTL 内
                // 拒绝重新拉取。
                let applied_to_catalog = self
                    .state
                    .chat_state
                    .replace_provider_models(&provider, models);
                let state = if applied_to_catalog {
                    crate::chat::ModelListState::Ready {
                        fetched_at: Instant::now(),
                    }
                } else {
                    crate::chat::ModelListState::Builtin
                };
                self.state.chat_state.set_model_list_state(&provider, state);
            }
            Ok(_) => {
                // 官方返回空数组：不清空列表，只记录失败。
                self.state.chat_state.set_model_list_state(
                    &provider,
                    crate::chat::ModelListState::Failed {
                        message: "the provider returned no models".to_string(),
                        failed_at: Instant::now(),
                        hint_only: false,
                    },
                );
            }
            Err(ModelFetchError::MissingApiKey(env)) => {
                self.state.chat_state.set_model_list_state(
                    &provider,
                    crate::chat::ModelListState::Failed {
                        message: format!("set {env}"),
                        failed_at: Instant::now(),
                        hint_only: true,
                    },
                );
            }
            Err(ModelFetchError::Unsupported) => {
                self.state
                    .chat_state
                    .set_model_list_state(&provider, crate::chat::ModelListState::Unsupported);
            }
            Err(ModelFetchError::Failed(message)) => {
                self.state.chat_state.set_model_list_state(
                    &provider,
                    crate::chat::ModelListState::Failed {
                        message,
                        failed_at: Instant::now(),
                        hint_only: false,
                    },
                );
            }
        }
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
    async fn execute_hook_action(&mut self, action: xiaoo_api::chat::HookAction) {
        match action {
            xiaoo_api::chat::HookAction::CreateSession { session_id } => {
                tracing::info!(
                    session_id = %session_id,
                    "TUI: hook action create_session — switching focus"
                );
                self.switch_to_remote_session(session_id).await;
            }
            xiaoo_api::chat::HookAction::SwitchSession { session_id } => {
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
            xiaoo_api::chat::HookAction::SendPrompt {
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

/// Coalesce a mouse-wheel (or pointer-motion) burst into a single redraw.
///
/// A wheel flick delivers 10–30 ScrollUp/ScrollDown events within a few
/// hundred milliseconds. Handling them one per loop iteration costs a full
/// frame each, and because scrolling shifts every visible row, every frame
/// repaints the entire transcript area — tens of KB per event once the
/// content is markdown-rich. On long transcripts that saturates the
/// terminal and scrolling stutters; a fresh conversation has nothing to
/// scroll (events are handled as clamped no-ops) and stays smooth, which
/// is why the lag only shows up once the Messages box holds a lot of
/// content.
///
/// After a handled wheel/motion event, drain the events already queued in
/// the tty buffer and apply their state changes inline — scroll updates
/// are O(1) and idempotent at the scroll boundary — letting the single
/// `terminal.draw` at the top of the loop repaint the whole burst at once.
/// The first non-coalescable event (key, click, …) is stashed in
/// `pending_event` so ordering with other input is preserved.
async fn coalesce_wheel_scroll_burst(
    app: &mut App,
    handled_event: &Event,
    pending_event: &mut Option<Event>,
) -> Result<()> {
    let coalescable = |event: &Event| {
        matches!(
            event,
            Event::Mouse(mouse)
                if matches!(
                    mouse.kind,
                    MouseEventKind::ScrollUp
                        | MouseEventKind::ScrollDown
                        | MouseEventKind::ScrollLeft
                        | MouseEventKind::ScrollRight
                        | MouseEventKind::Moved
                )
        )
    };
    if !coalescable(handled_event) {
        return Ok(());
    }

    // Drain queued events with the synchronous crossterm API, never the
    // `EventStream`. `EventStream::poll_next` must be driven by an
    // executor-registered waker: polling it with `now_or_never()` passes a
    // noop waker, and when no event is buffered crossterm arms its
    // background poll thread with that noop waker while the thread parks
    // holding the global event-reader mutex. The main `select!` can then
    // neither see queued events (`try_lock` fails → "no events") nor re-arm
    // the stream with the real waker, so every subsequent input event is
    // only noticed when an unrelated `select!` branch fires (the 250ms idle
    // sleep). The sync `poll`/`read` pair shares the same reader state but
    // never touches a waker, so the stream's wakeup chain stays intact.
    for _ in 0..128 {
        if !crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
            break;
        }
        let Ok(event) = crossterm::event::read() else {
            break;
        };
        if !coalescable(&event) {
            *pending_event = Some(event);
            return Ok(());
        }
        app.handle_event(event).await?;
    }
    Ok(())
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
