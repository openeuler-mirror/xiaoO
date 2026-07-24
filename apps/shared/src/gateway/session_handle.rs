use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionInputKind, SessionLeaseTable,
    SessionLifecycleStatus, SessionRecord, SessionServiceError, SessionSubmitReceipt,
};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::session_supervisor::SessionSupervisor;

const SESSION_COMMAND_QUEUE_CAPACITY: usize = 16;

#[derive(Clone)]
pub(crate) struct SessionHandle {
    session_id: String,
    tx: mpsc::Sender<SessionCommand>,
    supervisor: Arc<SessionSupervisor>,
    status_rx: watch::Receiver<SessionHandleStatus>,
    queue_depth: Arc<AtomicUsize>,
    /// Atomic "closing" flag shared with the actor. Set by the orphan reaper
    /// (via [`mark_closing`]) before `force_close_session_inner` so a turn
    /// popped between the reaper's `phase` check and the `ForceClose` command
    /// is re-queued (not rejected) until the reaper's TOCTOU re-check
    /// resolves. Also consulted by `try_increment_queue_depth` so new
    /// `run_turn` calls fail fast with `SessionClosed`.
    closing: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionHandleStatus {
    pub session_id: String,
    pub lifecycle: SessionLifecycleStatus,
    pub phase: SessionPhase,
    pub active_turn_id: Option<uuid::Uuid>,
    pub queue_depth: usize,
    pub last_active_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionPhase {
    Idle,
    Running,
    Paused,
    Closing,
    Closed,
}

pub(crate) enum SessionCommand {
    RunTurn {
        request: AppTurnRequest,
        resolved_runtime: ResolvedSessionRuntime,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
        reply: oneshot::Sender<Result<AppTurnResult, SessionServiceError>>,
    },
    Snapshot {
        reply: oneshot::Sender<Result<SessionRecord, SessionServiceError>>,
    },
    CancelActiveTurn {
        reply: oneshot::Sender<Result<SessionSubmitReceipt, SessionServiceError>>,
    },
    ForceClose {
        reply: oneshot::Sender<Result<SessionRecord, SessionServiceError>>,
    },
    HibernateIdle {
        idle_before_ms: u64,
        reply: oneshot::Sender<Result<Option<SessionRecord>, SessionServiceError>>,
    },
    /// No-op wakeup sent by `clear_closing` after the reaper's TOCTOU
    /// re-check undid `mark_closing`. The actor's `run` loop processes it
    /// as a no-op and loops back to `start_next_turn_if_possible`, which
    /// retries any turns re-queued during the `closing` window.
    Nop,
}

struct RunTurnCommand {
    request: AppTurnRequest,
    resolved_runtime: ResolvedSessionRuntime,
    event_sink: Option<Arc<dyn LoopEventSink>>,
    interaction_handle: Option<Arc<dyn InteractionHandle>>,
    channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    reply: oneshot::Sender<Result<AppTurnResult, SessionServiceError>>,
}

struct ActiveTurn {
    turn_id: uuid::Uuid,
    cancel: CancellationToken,
}

enum ActorEvent {
    Command(Option<SessionCommand>),
    ActiveDone,
}

impl SessionHandle {
    pub(crate) async fn new(
        session_id: String,
        supervisor: Arc<SessionSupervisor>,
        lease_table: SessionLeaseTable,
    ) -> Self {
        let snapshot = supervisor.snapshot().await;
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let initial_status = SessionHandleStatus {
            session_id: session_id.clone(),
            lifecycle: snapshot.status,
            phase: SessionPhase::Idle,
            active_turn_id: None,
            queue_depth: 0,
            last_active_at_ms: snapshot.updated_at_ms,
        };
        let (tx, rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
        let (status_tx, status_rx) = watch::channel(initial_status);
        let actor = SessionActor {
            session_id: session_id.clone(),
            supervisor: supervisor.clone(),
            rx,
            status_tx,
            queue_depth: queue_depth.clone(),
            pending_turns: VecDeque::new(),
            active_turn: None,
            active_done_rx: None,
            phase: SessionPhase::Idle,
            close_reply: None,
            lease_table,
            closing: closing.clone(),
        };
        tokio::spawn(actor.run());

        Self {
            session_id,
            tx,
            supervisor,
            status_rx,
            queue_depth,
            closing,
        }
    }

    pub(crate) fn supervisor(&self) -> Arc<SessionSupervisor> {
        Arc::clone(&self.supervisor)
    }

    /// Mark this handle as closing so subsequent `run_turn` /
    /// `start_next_turn_if_possible` reject new turns atomically. Used by the
    /// orphan reaper to close the race window between its `phase` check and
    /// the `ForceClose` command being processed by the actor.
    pub(crate) fn mark_closing(&self) {
        self.closing
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Clear the `closing` flag set by [`mark_closing`]. Used by the orphan
    /// reaper's TOCTOU re-check: if a heartbeat refreshed the lease after
    /// `mark_closing`, the reaper skips this session and must undo
    /// `mark_closing` so the handle is not bricked. Sends a `Nop` wakeup so
    /// turns re-queued during the `closing` window are retried promptly
    /// instead of waiting for the next external command.
    pub(crate) fn clear_closing(&self) {
        self.closing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = self.tx.try_send(SessionCommand::Nop);
    }

    pub(crate) async fn run_turn(
        &self,
        request: AppTurnRequest,
        resolved_runtime: ResolvedSessionRuntime,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.try_increment_queue_depth()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        let command = SessionCommand::RunTurn {
            request,
            resolved_runtime,
            event_sink,
            interaction_handle,
            channel_file_sender,
            reply: reply_tx,
        };

        if let Err(error) = self.tx.try_send(command) {
            self.decrement_queue_depth();
            return match error {
                mpsc::error::TrySendError::Full(_) => Err(SessionServiceError::SessionBusy {
                    session_id: self.session_id.clone(),
                    message: "session command queue is full".to_string(),
                }),
                mpsc::error::TrySendError::Closed(_) => Err(SessionServiceError::SessionClosed {
                    session_id: self.session_id.clone(),
                }),
            };
        }

        reply_rx
            .await
            .map_err(|_| SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            })?
    }

    pub(crate) async fn snapshot(&self) -> Result<SessionRecord, SessionServiceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::Snapshot { reply })
            .await
            .map_err(|_| SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            })?;
        rx.await.map_err(|_| SessionServiceError::SessionClosed {
            session_id: self.session_id.clone(),
        })?
    }

    pub(crate) async fn cancel_active_turn(
        &self,
    ) -> Result<SessionSubmitReceipt, SessionServiceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::CancelActiveTurn { reply })
            .await
            .map_err(|_| SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            })?;
        rx.await.map_err(|_| SessionServiceError::SessionClosed {
            session_id: self.session_id.clone(),
        })?
    }

    pub(crate) async fn force_close(&self) -> Result<SessionRecord, SessionServiceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::ForceClose { reply })
            .await
            .map_err(|_| SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            })?;
        rx.await.map_err(|_| SessionServiceError::SessionClosed {
            session_id: self.session_id.clone(),
        })?
    }

    pub(crate) async fn hibernate_idle(
        &self,
        idle_before_ms: u64,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::HibernateIdle {
                idle_before_ms,
                reply,
            })
            .await
            .map_err(|_| SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            })?;
        rx.await.map_err(|_| SessionServiceError::SessionClosed {
            session_id: self.session_id.clone(),
        })?
    }

    pub(crate) fn status(&self) -> SessionHandleStatus {
        self.status_rx.borrow().clone()
    }

    fn try_increment_queue_depth(&self) -> Result<(), SessionServiceError> {
        // Reject before queueing when the handle is closing (e.g. orphan
        // reaper), avoiding the window where the actor pops and starts the
        // turn before `ForceClose` arrives.
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        let mut current = self.queue_depth.load(Ordering::SeqCst);
        loop {
            if current >= SESSION_COMMAND_QUEUE_CAPACITY {
                return Err(SessionServiceError::SessionBusy {
                    session_id: self.session_id.clone(),
                    message: "session root turn queue is full".to_string(),
                });
            }
            match self.queue_depth.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(()),
                Err(next) => current = next,
            }
        }
    }

    fn decrement_queue_depth(&self) {
        self.queue_depth.fetch_sub(1, Ordering::SeqCst);
    }
}

struct SessionActor {
    session_id: String,
    supervisor: Arc<SessionSupervisor>,
    rx: mpsc::Receiver<SessionCommand>,
    status_tx: watch::Sender<SessionHandleStatus>,
    queue_depth: Arc<AtomicUsize>,
    pending_turns: VecDeque<RunTurnCommand>,
    active_turn: Option<ActiveTurn>,
    active_done_rx: Option<oneshot::Receiver<()>>,
    phase: SessionPhase,
    close_reply: Option<oneshot::Sender<Result<SessionRecord, SessionServiceError>>>,
    /// Shared attach-lease table. Consulted at pop-time to fail-fast any
    /// queued turn whose `client_id` is no longer the current lease holder
    /// (e.g. lease was taken over while this turn was waiting). Read-only
    /// here; writes go through `CoreBackedSessionService` (`acquire` /
    /// `heartbeat` / `detach` / `remove`).
    lease_table: SessionLeaseTable,
    /// Atomic closing flag shared with `SessionHandle`. Consulted at
    /// pop-time by `next_runnable_turn`: when set, popped turns are
    /// re-queued (`push_front`) and the actor pauses popping until a `Nop`
    /// wakeup from `clear_closing` (TOCTOU recovery) or `ForceClose`
    /// (`fail_pending_turns` drain) arrives.
    closing: Arc<std::sync::atomic::AtomicBool>,
}

impl SessionActor {
    async fn run(mut self) {
        loop {
            self.start_next_turn_if_possible().await;

            if self.active_done_rx.is_some() {
                let event = {
                    let active_done = self
                        .active_done_rx
                        .as_mut()
                        .expect("active_done_rx should be present");
                    tokio::select! {
                        done = active_done => {
                            if done.is_err() {
                                tracing::warn!(
                                    session_id = %self.session_id,
                                    "active turn completion sender dropped"
                                );
                            }
                            ActorEvent::ActiveDone
                        }
                        command = self.rx.recv() => ActorEvent::Command(command),
                    }
                };
                if !self.handle_event(event).await {
                    break;
                }
            } else {
                match self.rx.recv().await {
                    Some(command) => {
                        if !self.handle_event(ActorEvent::Command(Some(command))).await {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    async fn handle_event(&mut self, event: ActorEvent) -> bool {
        match event {
            ActorEvent::Command(Some(command)) => {
                self.handle_command(command).await;
                true
            }
            ActorEvent::Command(None) => false,
            ActorEvent::ActiveDone => {
                self.active_turn = None;
                self.active_done_rx = None;
                if let Some(reply) = self.close_reply.take() {
                    self.finish_force_close(reply).await;
                } else if self.phase != SessionPhase::Closed {
                    self.phase = SessionPhase::Idle;
                    self.publish_status(None).await;
                }
                true
            }
        }
    }

    async fn handle_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::RunTurn {
                request,
                resolved_runtime,
                event_sink,
                interaction_handle,
                channel_file_sender,
                reply,
            } => {
                if matches!(
                    self.phase,
                    SessionPhase::Paused | SessionPhase::Closing | SessionPhase::Closed
                ) {
                    self.queue_depth.fetch_sub(1, Ordering::SeqCst);
                    let _ = reply.send(Err(SessionServiceError::SessionClosed {
                        session_id: self.session_id.clone(),
                    }));
                    self.publish_status(self.active_turn.as_ref().map(|turn| turn.turn_id))
                        .await;
                    return;
                }
                self.pending_turns.push_back(RunTurnCommand {
                    request,
                    resolved_runtime,
                    event_sink,
                    interaction_handle,
                    channel_file_sender,
                    reply,
                });
                self.publish_status(self.active_turn.as_ref().map(|turn| turn.turn_id))
                    .await;
            }
            SessionCommand::Snapshot { reply } => {
                let _ = reply.send(Ok(self.supervisor.snapshot().await));
            }
            SessionCommand::CancelActiveTurn { reply } => {
                if let Some(active) = self.active_turn.as_ref() {
                    active.cancel.cancel();
                }
                let _ = reply.send(Ok(SessionSubmitReceipt {
                    session_id: self.session_id.clone(),
                    accepted_kind: SessionInputKind::CancelActiveTurn,
                }));
            }
            SessionCommand::ForceClose { reply } => {
                self.begin_force_close(reply).await;
            }
            SessionCommand::HibernateIdle {
                idle_before_ms,
                reply,
            } => {
                if self.phase != SessionPhase::Idle
                    || self.active_turn.is_some()
                    || !self.pending_turns.is_empty()
                    || self.queue_depth.load(Ordering::SeqCst) > 0
                {
                    let _ = reply.send(Ok(None));
                    return;
                }
                let snapshot = self.supervisor.snapshot().await;
                if snapshot.updated_at_ms > idle_before_ms {
                    let _ = reply.send(Ok(None));
                    return;
                }
                let paused = self.supervisor.hibernate_idle().await;
                self.phase = SessionPhase::Paused;
                self.publish_status(None).await;
                let _ = reply.send(Ok(Some(paused)));
            }
            SessionCommand::Nop => {
                // No-op wakeup: the command itself is the signal. The `run`
                // loop will call `start_next_turn_if_possible` on the next
                // iteration, retrying any turns re-queued during the
                // `closing` window after `clear_closing`.
            }
        }
    }

    /// Pop the next queued turn whose submitter is still the current lease
    /// holder (or a daemon-internal principal, which bypasses the holder
    /// check). Ineligible turns (lease taken over, or the actor is shutting
    /// down) are rejected in-place (their `reply` is sent an `Err`) and the
    /// loop continues. Returns `None` when the actor is busy / closing /
    /// closed, the queue is exhausted, **or** the reaper has set `closing`
    /// (turns are re-queued, not rejected — see below).
    ///
    /// The active turn is NOT interrupted — killing an in-flight LLM / tool
    /// call mid-stream would leave the session half-applied. The reaper's
    /// `mark_closing` + `ForceClose` sequence is the only path that halts a
    /// running turn.
    ///
    /// **`closing` window**: when the reaper sets `closing` it may still
    /// undo it via `clear_closing` (TOCTOU re-check). Turns popped during
    /// this window are **re-queued** (`push_front`) rather than rejected so
    /// the reaper's TOCTOU recovery doesn't leave a spurious
    /// `SessionClosed` for a turn that should have run. `clear_closing`
    /// sends a `Nop` wakeup so the re-queued turn is retried; if the reaper
    /// instead proceeds to `ForceClose`, `fail_pending_turns` drains it.
    async fn next_runnable_turn(&mut self) -> Option<RunTurnCommand> {
        loop {
            if self.active_turn.is_some()
                || matches!(
                    self.phase,
                    SessionPhase::Paused | SessionPhase::Closing | SessionPhase::Closed
                )
            {
                return None;
            }
            let Some(next) = self.pending_turns.pop_front() else {
                return None;
            };
            // Reaper `closing` flag: re-queue and pause popping rather than
            // rejecting. The reaper's TOCTOU re-check may clear `closing`
            // (→ `Nop` wakeup retries the turn) or proceed to `ForceClose`
            // (→ `fail_pending_turns` drains it). Either way the turn is
            // not spuriously rejected during the race window.
            if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
                self.pending_turns.push_front(next);
                return None;
            }
            // Pop-time eligibility check: lease holder check. On reject the
            // helper returns the error to send on the turn's `reply`; this
            // loop does the accounting + reply plumbing.
            if let Some(error) = self.should_reject_at_pop_time(&next).await {
                self.queue_depth.fetch_sub(1, Ordering::SeqCst);
                let _ = next.reply.send(Err(error));
                continue;
            }
            return Some(next);
        }
    }

    /// Pop-time eligibility check for a queued turn. Returns `Some(error)`
    /// when the turn must be rejected, or `None` when it may start now. Pure
    /// decision — does NOT touch `queue_depth` or `reply` (the caller owns
    /// the accounting so it lives in exactly one place).
    ///
    /// Reject reason: the submitter's `client_id` is no longer the holder
    /// (→ `SessionAttachedByAnotherClient`). Daemon-internal principals
    /// (`daemon:*`) bypass the holder check explicitly. The `closing` flag
    /// is NOT checked here — it's handled by `next_runnable_turn`'s
    /// re-queue path so the reaper's TOCTOU recovery doesn't spuriously
    /// reject turns.
    async fn should_reject_at_pop_time(
        &mut self,
        next: &RunTurnCommand,
    ) -> Option<SessionServiceError> {
        let Some(client_id) = next.request.client_id.as_deref().filter(|s| !s.is_empty()) else {
            // Anonymous / legacy caller — router-layer `assert_lease_holder`
            // already gated it (or the rollout policy allows it).
            return None;
        };
        // Daemon-internal principals bypass the pop-time holder check — they're
        // cooperative background callers that never hold a lease.
        if crate::gateway::is_daemon_principal(client_id) {
            return None;
        }
        // Read-only holder check: the pop-time guard must NOT have the
        // stale-takeover side effect (which would steal the lease from a
        // freshly-staled holder). A stale lease returns Ok; the router-level
        // guard already did the takeover at submit time.
        if let Err(failure) = self
            .lease_table
            .check_holder(&self.session_id, Some(client_id))
            .await
        {
            return Some(SessionServiceError::from_lease_check_failure(
                &self.session_id,
                failure,
            ));
        }
        None
    }

    async fn start_next_turn_if_possible(&mut self) {
        // Pop turns until we find one whose submitter still holds the lease;
        // turns queued before a takeover are fail-fast'd with
        // `SessionAttachedByAnotherClient`. Turns popped while the reaper's
        // `closing` flag is set are re-queued (not rejected) so the reaper's
        // TOCTOU recovery via `clear_closing` can still let them run.
        let Some(turn) = self.next_runnable_turn().await else {
            return;
        };

        self.queue_depth.fetch_sub(1, Ordering::SeqCst);
        let turn_id = uuid::Uuid::new_v4();
        // Use the externally-provided cancel token (from the TUI via bindings)
        // when available so Esc can actually cancel the backend turn; fall back
        // to a fresh token for non-TUI callers (CLI, daemon, subagents).
        let cancel = turn
            .resolved_runtime
            .bindings
            .cancel_token
            .clone()
            .unwrap_or_else(CancellationToken::new);
        let (done_tx, done_rx) = oneshot::channel();
        let supervisor = self.supervisor.clone();
        let cancel_for_task = cancel.clone();
        self.active_turn = Some(ActiveTurn { turn_id, cancel });
        self.active_done_rx = Some(done_rx);
        self.phase = SessionPhase::Running;
        self.publish_status(Some(turn_id)).await;

        tokio::spawn(async move {
            supervisor
                .prepare_root_turn(&turn.request, &turn.resolved_runtime)
                .await;
            let result = supervisor
                .run_root_turn(
                    turn.request,
                    turn.resolved_runtime,
                    turn.event_sink,
                    turn.interaction_handle,
                    turn.channel_file_sender,
                    Some(cancel_for_task),
                )
                .await;
            let _ = turn.reply.send(result);
            let _ = done_tx.send(());
        });
    }

    async fn begin_force_close(
        &mut self,
        reply: oneshot::Sender<Result<SessionRecord, SessionServiceError>>,
    ) {
        if self.phase == SessionPhase::Closed {
            let _ = reply.send(Ok(self.supervisor.snapshot().await));
            return;
        }

        self.phase = SessionPhase::Closing;
        if let Some(active) = self.active_turn.as_ref() {
            active.cancel.cancel();
        }
        self.fail_pending_turns();
        self.publish_status(self.active_turn.as_ref().map(|turn| turn.turn_id))
            .await;

        if self.active_turn.is_some() {
            if self.close_reply.is_some() {
                let _ = reply.send(Err(SessionServiceError::SessionBusy {
                    session_id: self.session_id.clone(),
                    message: "session is already closing".to_string(),
                }));
            } else {
                self.close_reply = Some(reply);
            }
            return;
        }

        self.finish_force_close(reply).await;
    }

    async fn finish_force_close(
        &mut self,
        reply: oneshot::Sender<Result<SessionRecord, SessionServiceError>>,
    ) {
        let closed = self.supervisor.force_close().await;
        self.phase = SessionPhase::Closed;
        self.publish_status(None).await;
        let _ = reply.send(Ok(closed));
    }

    fn fail_pending_turns(&mut self) {
        while let Some(turn) = self.pending_turns.pop_front() {
            self.queue_depth.fetch_sub(1, Ordering::SeqCst);
            let _ = turn.reply.send(Err(SessionServiceError::SessionClosed {
                session_id: self.session_id.clone(),
            }));
        }
    }

    async fn publish_status(&self, active_turn_id: Option<uuid::Uuid>) {
        let snapshot = self.supervisor.snapshot().await;
        let lifecycle = match self.phase {
            SessionPhase::Idle => snapshot.status.clone(),
            SessionPhase::Running | SessionPhase::Closing => SessionLifecycleStatus::Running,
            SessionPhase::Paused => SessionLifecycleStatus::Paused,
            SessionPhase::Closed => SessionLifecycleStatus::Closed,
        };
        let _ = self.status_tx.send(SessionHandleStatus {
            session_id: self.session_id.clone(),
            lifecycle,
            phase: self.phase,
            active_turn_id,
            queue_depth: self.queue_depth.load(Ordering::SeqCst),
            last_active_at_ms: snapshot.updated_at_ms,
        });
    }
}
