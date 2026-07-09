use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionInputKind,
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
    pub(crate) async fn new(session_id: String, supervisor: Arc<SessionSupervisor>) -> Self {
        let snapshot = supervisor.snapshot().await;
        let queue_depth = Arc::new(AtomicUsize::new(0));
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
        };
        tokio::spawn(actor.run());

        Self {
            session_id,
            tx,
            supervisor,
            status_rx,
            queue_depth,
        }
    }

    pub(crate) fn supervisor(&self) -> Arc<SessionSupervisor> {
        Arc::clone(&self.supervisor)
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

    #[allow(dead_code)]
    pub(crate) fn status(&self) -> SessionHandleStatus {
        self.status_rx.borrow().clone()
    }

    fn try_increment_queue_depth(&self) -> Result<(), SessionServiceError> {
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
                if matches!(self.phase, SessionPhase::Closing | SessionPhase::Closed) {
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
        }
    }

    async fn start_next_turn_if_possible(&mut self) {
        if self.active_turn.is_some()
            || matches!(self.phase, SessionPhase::Closing | SessionPhase::Closed)
        {
            return;
        }

        let Some(turn) = self.pending_turns.pop_front() else {
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
