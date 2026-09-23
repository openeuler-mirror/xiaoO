use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionControlPlane,
    SessionDetachRequest, SessionHeartbeatRequest, SessionInput, SessionLifecycleStatus,
    SessionOpenRequest, SessionRecord, SessionRuntimeBuildInput, SessionRuntimeResolveError,
    SessionRuntimeResolver, SessionService, SessionServiceError, SessionStateOutcome, SessionStore,
    SessionStoreError,
};
use crate::{
    RuntimeCheckoutRequest, RuntimeCheckoutResult, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, RuntimeCheckpointSnapshotDeleteRequest,
    RuntimeCheckpointSnapshotDeleteResult, RuntimeExecRequest, RuntimeExecResult,
    RuntimePauseRequest, RuntimePauseResult, RuntimeReadFileRequest, RuntimeReadFileResult,
    RuntimeRecord, RuntimeResumeRequest, RuntimeResumeResult, RuntimeWriteFileRequest,
    RuntimeWriteFileResult,
};
use agent_contracts::backend::{
    capability::{
        exec::ExecRequest,
        filesystem::{ReadBytesRequest, WriteBytesRequest, WriteMode},
    },
    BackendPath,
};
use agent_contracts::{ChannelFileSender, HookerRegistry, InteractionHandle, LoopEventSink};
use agent_types::common::{workspace_root_string, HookerId};
use agent_types::hook::{HookAction, HookInvokeInput, HookInvokeMetadata, HookPointId};
use agent_types::session::{
    SessionClosedHookInput, SessionCreatedHookInput, SessionStateHookInput,
};
use async_trait::async_trait;
use base64::Engine as _;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use subagent::{
    JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControl, SubagentControlError,
};
use tokio::sync::Mutex;
use xiaoo_api::runtime::NoopRuntimeView;

use super::memory_automation::{
    render_memory_context, CompletedTurnIngest, TurnMemoryAutomation, TurnMemoryContext,
};
use super::session_backend::{
    checkout_backend_with_eviction, lease_session_backend, sync_session_backend_instance,
    CheckoutEvictionContext,
};
use super::session_handle::SessionHandle;
use super::session_lease::{is_daemon_principal, LeaseAcquireOutcome, SessionLeaseTable};
use super::session_supervisor::SessionSupervisor;
use crate::backend::{
    BackendCheckoutRequest, BackendCheckpointRequest, BackendCheckpointSnapshotDeleteRequest,
    BackendError, BackendInfo, BackendLease, BackendListFilter, BackendManager,
};
use crate::runtime_checkpoint::{InMemoryRuntimeCheckpointStore, RuntimeCheckpoint};

/// Overall wall-clock budget for collecting `*.Session.lifecycle.state`
/// hook actions before the turn's `Done` event is emitted. Bounded at one
/// hooker's per-subprocess cap (`PLUGIN_HOOK_COMMAND_TIMEOUT_MS = 30s`) so the
/// sum across N hookers is capped at 30s (not N × 30s). On timeout the
/// spawned task is aborted (in-flight subprocess reaped via `kill_on_drop`).
const SESSION_STATE_HOOK_OVERALL_DEADLINE: tokio::time::Duration =
    tokio::time::Duration::from_secs(30);
const RUNTIME_EXEC_FALLBACK_SHELL: &str = "/bin/sh";

fn resolve_runtime_exec_shell(requested: Option<String>, backend_default: Option<&str>) -> String {
    requested
        .or_else(|| backend_default.map(str::to_string))
        .unwrap_or_else(|| RUNTIME_EXEC_FALLBACK_SHELL.to_string())
}

/// Render a session record's workspace root as the optional string carried
/// by session lifecycle hook inputs. Session hooks are dispatched with a
/// `NoopRuntimeView`, so the session record is the only workspace source
/// available. The empty-root-to-`None` rule is defined once in
/// [`workspace_root_string`] and shared with the plugin payload builders,
/// so the null semantics cannot drift between the two dispatch paths.
fn session_workspace_for_hook(workspace_root: &std::path::Path) -> Option<String> {
    workspace_root_string(workspace_root)
}

/// Identity + transition fields shared by the two `*.Session.lifecycle.state`
/// dispatch paths (awaited action collection / background fire-and-forget).
pub(crate) struct SessionStateHookEvent {
    session_id: String,
    sender_id: String,
    agent_id: String,
    state: String,
    outcome: String,
    workspace: Option<String>,
}

pub(crate) struct CoreBackedSessionService {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    sessions_handler: Mutex<HashMap<String, SessionHandle>>,
    runtime_initialization_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    hooker_registry: Arc<dyn HookerRegistry>,
    backend_manager: Arc<BackendManager>,
    runtime_checkpoints: InMemoryRuntimeCheckpointStore,
    /// Cross-turn `send_prompt` chain depth cap (exclusive upper bound on
    /// `chain_depth`): `N` permits N turns total — the user-initiated turn
    /// (depth `0`) plus `N - 1` `send_prompt`-triggered turns (depths
    /// `1..=N-1`). Defaults to
    /// [`DEFAULT_MAX_PROMPT_CHAIN_DEPTH`](agent_types::hook::DEFAULT_MAX_PROMPT_CHAIN_DEPTH)
    /// (128); configurable via `[hooker].max_prompt_chain_depth`.
    max_prompt_chain_depth: usize,
    /// Per-session attach lease table. See [`SessionLeaseTable`]: acquired at
    /// the top of `open_session`, refreshed by `heartbeat_session`, released
    /// (without destroying the session) by `detach_session`, checked on every
    /// turn-driving RPC by `assert_lease_holder`.
    sessions_lease: SessionLeaseTable,
    /// When `true`, mutating RPCs that omit `client_id` are rejected with
    /// [`SessionServiceError::LeaseRequired`]. Defaults to `false` for
    /// gradual rollout; flip via [`Self::set_enforce_anonymous_lease`]
    /// (typically driven by `XIAOO_ENFORCE_LEASE` in `AppBootstrap`).
    enforce_anonymous_lease: Arc<std::sync::atomic::AtomicBool>,
    /// Cap on how long a forwarded subagent interaction
    /// (`ask_user_question`) may wait for the user. Set by the daemon via
    /// [`Self::set_interaction_timeout`] (it forwards the same
    /// `interaction_timeout_secs` used for the HTTP router). Stored as
    /// whole seconds in an `AtomicU64` (0 = `None`); sub-second precision
    /// is dropped because the only caller uses `Duration::from_secs`.
    interaction_timeout: Arc<std::sync::atomic::AtomicU64>,
    memory_automation: Option<Arc<dyn TurnMemoryAutomation>>,
}

impl CoreBackedSessionService {
    pub(crate) fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_registry: Arc<dyn HookerRegistry>,
        backend_manager: Arc<BackendManager>,
        max_prompt_chain_depth: usize,
        memory_automation: Option<Arc<dyn TurnMemoryAutomation>>,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            sessions_handler: Mutex::new(HashMap::new()),
            runtime_initialization_locks: Mutex::new(HashMap::new()),
            hooker_registry,
            backend_manager,
            runtime_checkpoints: InMemoryRuntimeCheckpointStore::default(),
            max_prompt_chain_depth,
            sessions_lease: SessionLeaseTable::new(),
            enforce_anonymous_lease: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interaction_timeout: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            memory_automation,
        }
    }

    /// Toggle strict lease enforcement for anonymous callers. When `true`,
    /// `assert_lease_holder` returns
    /// [`SessionServiceError::LeaseRequired`] for any RPC whose body omits
    /// `client_id`. Idempotent; safe to call at any time (atomic store).
    pub(crate) fn set_enforce_anonymous_lease(&self, enabled: bool) {
        self.enforce_anonymous_lease
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    /// Read-side helper for `assert_lease_holder` and the bootstrap startup
    /// log. Pure for testability.
    pub(crate) fn anonymous_lease_enforced(&self) -> bool {
        self.enforce_anonymous_lease
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Set the cap on how long a forwarded subagent interaction may wait
    /// for the user. Applied to subsequently created `SessionSupervisor`
    /// handles (existing supervisors keep their creation-time value).
    /// `None` (the default) disables the outer cap; the supervisor then
    /// relies on the handle's own timeout or blocks until the user replies.
    pub(crate) fn set_interaction_timeout(&self, timeout: Option<std::time::Duration>) {
        let secs = timeout.map(|d| d.as_secs()).unwrap_or(0);
        self.interaction_timeout
            .store(secs, std::sync::atomic::Ordering::Release);
    }

    /// Spawn the background orphan-session reaper. Wakes every
    /// [`REAPER_INTERVAL`](crate::gateway::REAPER_INTERVAL) and force-closes
    /// sessions whose lease has been gone for longer than
    /// [`ORPHAN_SESSION_THRESHOLD_MS`](crate::gateway::ORPHAN_SESSION_THRESHOLD_MS)
    /// and whose last activity is older than the same threshold, reclaiming
    /// leaked backends (e2b sandboxes) when a TUI crashes
    /// and nobody comes back.
    ///
    /// The reaper must NOT close sessions whose in-memory handle is currently
    /// running an active turn (killing the sandbox mid-turn would leave the
    /// session half-applied).
    ///
    /// Returns a `JoinHandle` (currently only the daemon's `main.rs`) so
    /// callers can `.abort()` it on shutdown. Errors are logged and never
    /// propagated (best-effort).
    pub(crate) fn spawn_orphan_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        const ORPHAN_THRESHOLD_MS: u64 = crate::gateway::ORPHAN_SESSION_THRESHOLD_MS;
        const STALE_LEASE_MS: u64 = crate::gateway::STALE_LEASE_THRESHOLD_MS;

        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(crate::gateway::REAPER_INTERVAL);
            // Skip the immediate first tick so a sweep doesn't fire before any
            // TUI has registered a lease.
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(error) = service
                    .run_reaper_sweep(ORPHAN_THRESHOLD_MS, STALE_LEASE_MS)
                    .await
                {
                    tracing::warn!(error = %error, "orphan session reaper sweep failed");
                }
            }
        })
    }

    /// Reaper's per-sweep logic, factored out for unit tests (tests drive it
    /// directly with backdated `updated_at_ms` rather than waiting 2h; no
    /// fake-clock injection — `now` uses [`crate::gateway::current_time_ms`],
    /// fail-closed on a skewed clock). Iterates every session record and
    /// delegates per-record close decisions to [`reap_one_record`].
    async fn run_reaper_sweep(
        &self,
        orphan_threshold_ms: u64,
        stale_threshold_ms: u64,
    ) -> Result<(), SessionServiceError> {
        // Use the lease table's fail-closed clock so a broken wall clock
        // doesn't reap live sessions (or never reap). Skip the sweep on clock
        // skew; the next tick retries once NTP corrects the clock.
        let Ok(now) = crate::gateway::session_lease::current_time_ms() else {
            tracing::warn!(
                "orphan session reaper: wall clock is before UNIX_EPOCH; \
                 skipping sweep (will retry next tick)"
            );
            return Ok(());
        };
        let all_sessions = self.session_store.list_all().await;
        let live_leases = self.sessions_lease.snapshot().await;

        for record in all_sessions {
            self.reap_one_record(
                &record,
                now,
                &live_leases,
                orphan_threshold_ms,
                stale_threshold_ms,
            )
            .await;
        }
        Ok(())
    }

    /// Per-record close decision for a single [`SessionRecord`] within a
    /// reaper sweep:
    ///
    /// * **Skip** — `Closed`, live lease, recent activity, running turn, or
    ///   lease refreshed between sweep snapshot and `mark_closing` (TOCTOU
    ///   re-check).
    /// * **Close** — `force_close_session_inner`; on failure
    ///   [`evict_session_artifacts`] runs so the session is not bricked.
    ///
    /// Skip-conditions are checked in order via [`should_skip_reap`]; the
    /// TOCTOU re-check reads the **current** lease table under its own mutex
    /// and undoes `mark_closing` via [`clear_closing`] if the lease was
    /// refreshed during the window.
    async fn reap_one_record(
        &self,
        record: &SessionRecord,
        now: u64,
        live_leases: &[(String, String, u64)],
        orphan_threshold_ms: u64,
        stale_threshold_ms: u64,
    ) {
        // All skip decisions (including the TOCTOU `mark_closing` undo) live
        // in `should_skip_reap` so this reads as "skip if asked, else close".
        if self
            .should_skip_reap(
                record,
                now,
                live_leases,
                orphan_threshold_ms,
                stale_threshold_ms,
            )
            .await
        {
            return;
        }

        let last_activity = record.updated_at_ms;
        tracing::info!(
            session_id = %record.session_id,
            last_activity_ms = last_activity,
            age_ms = now.saturating_sub(last_activity),
            "orphan session reaper: force-closing stale session"
        );
        // `close_and_evict_on_err` runs `force_close_session_inner` and, on
        // failure, evicts residual artifacts so a stuck `Closing` handle
        // can't brick the session. `mark_closing` was set in
        // `should_skip_reap` above; the error is logged here (not inside the
        // helper) so the `orphan session reaper:` prefix shows in the log.
        if let Err(error) = self.close_and_evict_on_err(&record.session_id).await {
            tracing::warn!(
                session_id = %record.session_id,
                error = %error,
                "orphan session reaper: force_close failed; residual artifacts evicted"
            );
        }
    }

    /// Skip-decision for [`reap_one_record`]: the four pure-read skip
    /// conditions (Closed / live-lease snapshot / recent activity / Running
    /// handle) plus the TOCTOU lease re-check that may undo `mark_closing`.
    ///
    /// Step 4 (Running check) and step 5 (TOCTOU re-check) both touch shared
    /// state — `mark_closing` is set in step 4 and may be undone by step 5 —
    /// so they live together here. The `sessions_handler` lock is held only
    /// for step 4 (phase check + `mark_closing`); step 5 reads the lease
    /// table under its own mutex. A `clear_closing` on the TOCTOU hit
    /// re-acquires `sessions_handler` briefly — the price of not holding it
    /// across the lease-table await (which would block every `run_turn`
    /// enqueue during the sweep).
    async fn should_skip_reap(
        &self,
        record: &SessionRecord,
        now: u64,
        live_leases: &[(String, String, u64)],
        orphan_threshold_ms: u64,
        stale_threshold_ms: u64,
    ) -> bool {
        // 1. Already closed — leave it untouched.
        if record.status == SessionLifecycleStatus::Closed {
            return true;
        }

        // 2. Lease still live (per sweep-start snapshot)?
        let has_live_lease = live_leases.iter().any(|(sid, _client_id, last_hb)| {
            *sid == record.session_id && now.saturating_sub(*last_hb) <= stale_threshold_ms
        });
        if has_live_lease {
            return true;
        }

        // 3. Stale activity?
        if now.saturating_sub(record.updated_at_ms) < orphan_threshold_ms {
            return true;
        }

        // 4. In-memory handle currently running a turn? Skip if Running.
        //    Hold the `sessions_handler` lock for the phase check AND
        //    `mark_closing` so they happen atomically. Without `mark_closing`,
        //    a turn popped from `pending_turns` between this check and the
        //    `ForceClose` command would be started mid-stream and then
        //    cancelled — the half-applied state the reaper avoids. Turns
        //    popped while `closing` is set are re-queued (not rejected) by
        //    `next_runnable_turn`, so a TOCTOU recovery via `clear_closing`
        //    (step 5) can still let them run.
        let can_close = self
            .sessions_handler
            .lock()
            .await
            .get(&record.session_id)
            .map(|handle| {
                if matches!(
                    handle.status().phase,
                    crate::gateway::session_handle::SessionPhase::Running
                ) {
                    false
                } else {
                    handle.mark_closing();
                    true
                }
            })
            .unwrap_or(true);
        if !can_close {
            return true;
        }

        // 5. TOCTOU re-check: the `live_leases` snapshot was taken at sweep
        //    start. A heartbeat between the snapshot and `mark_closing` above
        //    could have refreshed a previously-stale lease. Read the CURRENT
        //    table state; if it's now live, undo `mark_closing` and skip.
        if self
            .sessions_lease
            .has_live_lease(&record.session_id, stale_threshold_ms)
            .await
        {
            tracing::info!(
                session_id = %record.session_id,
                "orphan session reaper: lease refreshed during sweep; skipping"
            );
            if let Some(handle) = self.sessions_handler.lock().await.get(&record.session_id) {
                handle.clear_closing();
            }
            return true;
        }

        false
    }

    async fn runtime_initialization_lock(&self, runtime_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.runtime_initialization_locks.lock().await;
        // The map itself owns one strong reference. Drop entries with no
        // active/waiting caller so arbitrary runtime IDs cannot grow it forever.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(
            locks
                .entry(runtime_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn fire_session_hooks(&self, input: HookInvokeInput, hook_point: HookPointId) {
        let hookers = self.enabled_hooker_ids_for(&hook_point);

        let noop_runtime = NoopRuntimeView::new();
        for hooker_id in hookers {
            if let Some(hooker) = self.hooker_registry.get(&hooker_id) {
                if let Err(err) = hooker.invoke(input.clone(), &noop_runtime).await {
                    tracing::warn!(
                        hooker_id = %hooker_id,
                        hook_point = %hook_point.0,
                        error = %err,
                        "session hook invocation failed"
                    );
                }
            }
        }
    }

    /// Fire the `*.Session.lifecycle.state` event hook, await all hookers,
    /// and collect the side-effect `actions` they request. Used by the
    /// daemon's `stream_session_input` path (via `run_turn_inner`) so plugin-
    /// requested actions are bundled into the turn's `Done` event — the TUI
    /// processes them when it receives `Done`.
    ///
    /// Bounded by [`SESSION_STATE_HOOK_OVERALL_DEADLINE`] (30s) regardless of
    /// how many hookers are registered — a single hooker can still take up to
    /// its 30s per-subprocess cap (no regression), but the sum across N is
    /// capped at 30s. On timeout the spawned task is aborted (in-flight
    /// subprocess reaped via `kill_on_drop`); no actions are returned
    /// (best-effort, matching the documented semantics that action failures
    /// never propagate to the hook caller).
    ///
    /// Actions are returned raw: daemon-side execution (e.g. `open_session`
    /// for `CreateSession`/`SwitchSession`) is the caller's responsibility
    /// (the HTTP router does this via `DaemonHookActionSink`).
    ///
    /// The hookers run inside a `tokio::spawn`d task awaited under the
    /// deadline. A panicking or cancelled plugin task surfaces as a
    /// `JoinError` here (rather than unwinding into `run_turn_inner` and
    /// tearing down the SSE connection); `JoinError` yields an empty action
    /// set so the turn result is still delivered.
    pub(crate) async fn fire_session_state_hook_and_collect_actions(
        &self,
        event: SessionStateHookEvent,
        emitting_turn_chain_depth: usize,
    ) -> Vec<HookAction> {
        let SessionStateHookEvent {
            session_id,
            sender_id,
            agent_id,
            state,
            outcome,
            workspace,
        } = event;
        let hook_point = session_lifecycle_hook_point(&agent_id, "state");
        let hooker_ids = self.enabled_hooker_ids_for(&hook_point);
        if hooker_ids.is_empty() {
            return Vec::new();
        }

        let max_depth = self.max_prompt_chain_depth;
        let registry = Arc::clone(&self.hooker_registry);
        let mut hook_task = tokio::spawn(async move {
            let noop_runtime = NoopRuntimeView::new();
            let input = HookInvokeInput::SessionState {
                input: SessionStateHookInput {
                    session_id,
                    sender_id,
                    agent_id,
                    state,
                    outcome,
                    workspace,
                },
                metadata: HookInvokeMetadata::default(),
            };

            let mut all_actions = Vec::new();
            for hooker_id in hooker_ids {
                let Some(hooker) = registry.get(&hooker_id) else {
                    continue;
                };
                match hooker.invoke(input.clone(), &noop_runtime).await {
                    Ok(invoke_output) => {
                        all_actions.extend(invoke_output.actions);
                    }
                    Err(error) => {
                        tracing::warn!(
                            hooker_id = %hooker_id,
                            hook_point = "session.lifecycle.state",
                            error = %error,
                            "session state hook invocation failed"
                        );
                    }
                }
            }
            all_actions
        });

        let deadline_sleep = tokio::time::sleep(SESSION_STATE_HOOK_OVERALL_DEADLINE);
        tokio::pin!(deadline_sleep);
        let collected: Vec<HookAction> = tokio::select! {
            task_result = &mut hook_task => match task_result {
                Ok(actions) => actions,
                Err(join_error) => {
                    tracing::warn!(
                        hook_point = "session.lifecycle.state",
                        error = %join_error,
                        "session state hook task did not complete \
                         (panic or runtime shutdown); returning no actions"
                    );
                    Vec::new()
                }
            },
            _ = &mut deadline_sleep => {
                tracing::warn!(
                    hook_point = "session.lifecycle.state",
                    deadline_secs = SESSION_STATE_HOOK_OVERALL_DEADLINE.as_secs(),
                    "session state hook collection exceeded overall deadline; \
                     aborting pending hookers and returning no actions"
                );
                hook_task.abort();
                Vec::new()
            }
        };

        // Stamp and cap `SendPrompt` actions before forwarding. The emitting
        // turn's depth is known here (the turn just finished); each surviving
        // `SendPrompt` carries `chain_depth = emitting_turn_depth + 1` so the
        // TUI can relay it back via `RuntimeTurnRequest.chain_depth`, letting
        // the daemon track the resulting turn's depth and re-enforce the cap
        // when that turn ends. Plugin-supplied `chain_depth` values are
        // overwritten unconditionally — plugins cannot forge a low depth to
        // bypass the cap. A normal user-typed turn carries `chain_depth = 0`,
        // which resets the chain.
        stamp_and_cap_send_prompt_actions(collected, emitting_turn_chain_depth, max_depth)
    }

    /// Fire-and-forget `*.Session.lifecycle.state` for states with no
    /// `AppTurnResult` to attach actions to (today: `"failed"` after a
    /// turn terminates with `Err`; `outcome = "error"`). Plugin actions
    /// are discarded; plugin errors are logged. Bounded by
    /// [`SESSION_STATE_HOOK_OVERALL_DEADLINE`], mirroring
    /// [`fire_session_state_hook_and_collect_actions`].
    fn fire_session_state_hook_background(&self, event: SessionStateHookEvent) {
        let SessionStateHookEvent {
            session_id,
            sender_id,
            agent_id,
            state,
            outcome,
            workspace,
        } = event;
        let hook_point = session_lifecycle_hook_point(&agent_id, "state");
        let hooker_ids = self.enabled_hooker_ids_for(&hook_point);
        if hooker_ids.is_empty() {
            return;
        }
        let registry = Arc::clone(&self.hooker_registry);
        tokio::spawn(async move {
            let noop_runtime = NoopRuntimeView::new();
            let input = HookInvokeInput::SessionState {
                input: SessionStateHookInput {
                    session_id,
                    sender_id,
                    agent_id,
                    state,
                    outcome,
                    workspace,
                },
                metadata: HookInvokeMetadata::default(),
            };
            // Overall deadline caps accumulated background runtime and
            // (via `kill_on_drop`) reaps any spawned plugin subprocess on drop.
            let loop_body = async {
                for hooker_id in hooker_ids {
                    let Some(hooker) = registry.get(&hooker_id) else {
                        continue;
                    };
                    match hooker.invoke(input.clone(), &noop_runtime).await {
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(
                                hooker_id = %hooker_id,
                                hook_point = "session.lifecycle.state",
                                error = %error,
                                "session state hook invocation failed"
                            );
                        }
                    }
                }
            };
            if tokio::time::timeout(SESSION_STATE_HOOK_OVERALL_DEADLINE, loop_body)
                .await
                .is_err()
            {
                tracing::warn!(
                    hook_point = "session.lifecycle.state",
                    deadline_secs = SESSION_STATE_HOOK_OVERALL_DEADLINE.as_secs(),
                    "session state hook background dispatch exceeded overall deadline; \
                     aborting pending hookers"
                );
            }
        });
    }

    /// Collect the (id)s of enabled hookers registered for `hook_point`,
    /// sorted by id for a stable execution order. Shared by both
    /// [`fire_session_hooks`] and [`fire_session_state_hook_and_collect_actions`].
    fn enabled_hooker_ids_for(&self, hook_point: &HookPointId) -> Vec<HookerId> {
        let mut ids: Vec<HookerId> = self
            .hooker_registry
            .list_for_hook_point(hook_point)
            .into_iter()
            .filter(|h| self.hooker_registry.is_enabled(h.id()))
            .map(|h| h.id().clone())
            .collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        ids
    }

    async fn get_or_create_session_handle(&self, session: SessionRecord) -> SessionHandle {
        if let Some(existing) = self
            .sessions_handler
            .lock()
            .await
            .get(&session.session_id)
            .cloned()
        {
            return existing.clone();
        }

        let interaction_timeout = {
            let secs = self
                .interaction_timeout
                .load(std::sync::atomic::Ordering::Acquire);
            if secs == 0 {
                None
            } else {
                Some(std::time::Duration::from_secs(secs))
            }
        };
        let supervisor = Arc::new(SessionSupervisor::new(
            self.session_store.clone(),
            self.runtime_resolver.clone(),
            Arc::clone(&self.backend_manager),
            session.clone(),
            interaction_timeout,
        ));
        let handle = SessionHandle::new(
            session.session_id.clone(),
            supervisor,
            self.sessions_lease.clone(),
        )
        .await;
        let mut sessions = self.sessions_handler.lock().await;
        if let Some(existing) = sessions.get(&session.session_id) {
            return existing.clone();
        }
        sessions.insert(session.session_id.clone(), handle.clone());
        handle
    }

    async fn handle_for_session(&self, session_id: &str) -> Option<SessionHandle> {
        if let Some(existing) = self.sessions_handler.lock().await.get(session_id).cloned() {
            return Some(existing);
        }

        let session = self.session_store.load(session_id).await?;
        if session.status == SessionLifecycleStatus::Paused {
            return None;
        }
        Some(self.get_or_create_session_handle(session).await)
    }

    async fn idle_session_snapshot(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        if let Some(handle) = self.sessions_handler.lock().await.get(session_id).cloned() {
            let status = handle.status();
            if status.phase != super::session_handle::SessionPhase::Idle || status.queue_depth > 0 {
                return Err(SessionServiceError::SessionBusy {
                    session_id: session_id.to_string(),
                    message: "runtime must be idle before checkpoint or checkout".to_string(),
                });
            }
            return handle.snapshot().await;
        }

        let Some(session) = self.session_store.load(session_id).await else {
            return Err(SessionServiceError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        };
        if session.status == SessionLifecycleStatus::Running {
            return Err(SessionServiceError::SessionBusy {
                session_id: session_id.to_string(),
                message: "runtime must be idle before checkpoint or checkout".to_string(),
            });
        }
        Ok(session)
    }

    fn map_backend_error(
        context: &str,
        session_id: &str,
        error: BackendError,
    ) -> SessionServiceError {
        match error {
            BackendError::ResourceLimitExceeded { message } => SessionServiceError::SessionBusy {
                session_id: session_id.to_string(),
                message,
            },
            error => SessionServiceError::RuntimeBuild {
                message: format!("{context}: {error}"),
            },
        }
    }

    async fn lease_bound_backend_for_idle_runtime(
        &self,
        runtime_id: &str,
    ) -> Result<BackendLease, SessionServiceError> {
        let session = self.idle_session_snapshot(runtime_id).await?;
        if session.status == SessionLifecycleStatus::Closed {
            return Err(SessionServiceError::SessionClosed {
                session_id: runtime_id.to_string(),
            });
        }
        self.backend_manager
            .lease_bound_session(runtime_id)
            .await
            .map_err(|error| Self::map_runtime_backend_error(runtime_id, error))
    }

    fn map_runtime_backend_error(runtime_id: &str, error: BackendError) -> SessionServiceError {
        match error {
            BackendError::NotFound { .. } => SessionServiceError::SessionNotFound {
                session_id: runtime_id.to_string(),
            },
            BackendError::UnsupportedBackend { kind } => {
                SessionServiceError::UnsupportedCapability {
                    capability: format!("runtime backend: {kind}"),
                }
            }
            error => SessionServiceError::RuntimeBuild {
                message: format!("runtime backend operation failed: {error}"),
            },
        }
    }

    async fn checkpoint_runtime_internal(
        &self,
        request: RuntimeCheckpointRequest,
    ) -> Result<RuntimeCheckpointResult, SessionServiceError> {
        let session = self.idle_session_snapshot(&request.runtime_id).await?;
        let backend_checkpoint = if let Some(parent_backend) = session.backend_instance.as_ref() {
            Some(
                self.backend_manager
                    .checkpoint_backend(BackendCheckpointRequest {
                        backend_id: Some(parent_backend.backend_id.0.clone()),
                        session_id: Some(request.runtime_id.clone()),
                        name: request.name.clone(),
                        metadata: request.metadata.clone(),
                    })
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!("failed to checkpoint runtime backend: {error}"),
                    })?,
            )
        } else {
            None
        };

        let parent_checkpoint_id = self
            .runtime_checkpoints
            .latest_for_runtime(&request.runtime_id)
            .await;
        let checkpoint_id = format!("rtcp_{}", uuid::Uuid::new_v4().simple());
        let created_at_ms = current_time_ms();
        let checkpoint = RuntimeCheckpoint {
            checkpoint_id: checkpoint_id.clone(),
            runtime_id: request.runtime_id.clone(),
            parent_checkpoint_id: parent_checkpoint_id.clone(),
            created_at_ms,
            metadata: request.metadata.clone(),
            name: request.name.clone(),
            session: session.clone(),
            backend_checkpoint: backend_checkpoint
                .as_ref()
                .map(|result| result.checkpoint.clone()),
        };
        self.runtime_checkpoints.save(checkpoint).await;

        Ok(RuntimeCheckpointResult {
            checkpoint_id,
            runtime: RuntimeRecord::from_session(&session),
            parent_checkpoint_id,
            created_at_ms,
            metadata: request.metadata,
            name: request.name,
        })
    }

    async fn checkout_runtime_internal(
        &self,
        request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutResult, SessionServiceError> {
        let checkpoint = self
            .runtime_checkpoints
            .load(&request.checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: format!("checkpoint:{}", request.checkpoint_id),
            })?;
        let _ = self.idle_session_snapshot(&checkpoint.runtime_id).await?;

        let child_runtime_id = format!(
            "{}:checkout:{}",
            checkpoint.runtime_id,
            uuid::Uuid::new_v4().simple()
        );
        if self.session_store.load(&child_runtime_id).await.is_some() {
            return Err(SessionServiceError::SessionBusy {
                session_id: child_runtime_id,
                message: "generated runtime already exists".to_string(),
            });
        }

        let backend_lease = if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone()
        {
            Some(
                checkout_backend_with_eviction(
                    self.backend_manager.as_ref(),
                    &child_runtime_id,
                    backend_checkpoint,
                    self.session_store.clone(),
                    request.metadata.clone(),
                    &CheckoutEvictionContext::runtime_checkout(),
                    request.options.clone(),
                    None,
                )
                .await?,
            )
        } else {
            None
        };

        let now_ms = current_time_ms();
        let mut child = checkpoint.session.clone();
        child.session_id = child_runtime_id.clone();
        child.parent_runtime_id = Some(checkpoint.runtime_id.clone());
        child.forked_from_checkpoint_id = Some(checkpoint.checkpoint_id.clone());
        if let Some(conversation_id) = request.conversation_id {
            child.conversation_id = conversation_id;
        }
        if let Some(sender_id) = request.sender_id {
            child.sender_id = sender_id;
        }
        child.status = SessionLifecycleStatus::Idle;
        child.backend_instance = backend_lease.map(|lease| lease.instance());
        child.last_error = None;
        child.created_at_ms = now_ms;
        child.updated_at_ms = now_ms;

        self.session_store.save(child.clone()).await;
        self.runtime_checkpoints
            .register_runtime_head(child.session_id.clone(), checkpoint.checkpoint_id.clone())
            .await;

        let hook_point = session_lifecycle_hook_point(&child.runtime.agent_id.0, "created");
        self.fire_session_hooks(
            HookInvokeInput::SessionCreated {
                input: SessionCreatedHookInput {
                    session_id: child.session_id.clone(),
                    sender_id: child.sender_id.clone(),
                    workspace: session_workspace_for_hook(&child.runtime.workspace_root),
                },
                metadata: HookInvokeMetadata::default(),
            },
            hook_point,
        )
        .await;
        self.get_or_create_session_handle(child.clone()).await;

        Ok(RuntimeCheckoutResult {
            checkpoint_id: checkpoint.checkpoint_id,
            source_runtime_id: checkpoint.runtime_id,
            runtime: RuntimeRecord::from_session(&child),
        })
    }

    async fn pause_runtime_internal(
        &self,
        request: RuntimePauseRequest,
    ) -> Result<RuntimePauseResult, SessionServiceError> {
        let session = self.idle_session_snapshot(&request.runtime_id).await?;
        if session.status == SessionLifecycleStatus::Closed {
            return Err(SessionServiceError::SessionClosed {
                session_id: request.runtime_id,
            });
        }
        if session.status == SessionLifecycleStatus::Paused {
            return Err(SessionServiceError::SessionBusy {
                session_id: request.runtime_id,
                message: "runtime is already paused".to_string(),
            });
        }

        let backend_checkpoint = if let Some(parent_backend) = session.backend_instance.as_ref() {
            Some(
                self.backend_manager
                    .checkpoint_backend(BackendCheckpointRequest {
                        backend_id: Some(parent_backend.backend_id.0.clone()),
                        session_id: Some(request.runtime_id.clone()),
                        name: request.name.clone(),
                        metadata: request.metadata.clone(),
                    })
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!(
                            "failed to checkpoint runtime backend before pause: {error}"
                        ),
                    })?,
            )
        } else {
            None
        };

        self.backend_manager
            .release_session(&request.runtime_id)
            .await
            .map_err(|error| SessionServiceError::RuntimeShutdown {
                message: format!("failed to release runtime backend during pause: {error}"),
            })?;

        let checkpoint_id = format!("rtcp_{}", uuid::Uuid::new_v4().simple());
        let created_at_ms = current_time_ms();
        let parent_checkpoint_id = self
            .runtime_checkpoints
            .latest_for_runtime(&request.runtime_id)
            .await;
        let mut paused = session.clone();
        paused.status = SessionLifecycleStatus::Paused;
        paused.backend_instance = None;
        paused.last_error = None;
        paused.updated_at_ms = created_at_ms;

        let checkpoint = RuntimeCheckpoint {
            checkpoint_id: checkpoint_id.clone(),
            runtime_id: request.runtime_id.clone(),
            parent_checkpoint_id,
            created_at_ms,
            metadata: request.metadata.clone(),
            name: request.name.clone(),
            session: paused.clone(),
            backend_checkpoint: backend_checkpoint
                .as_ref()
                .map(|result| result.checkpoint.clone()),
        };
        self.runtime_checkpoints.save(checkpoint).await;
        self.runtime_checkpoints
            .register_paused_runtime(request.runtime_id.clone(), checkpoint_id.clone())
            .await;
        self.session_store.save(paused.clone()).await;
        self.sessions_handler
            .lock()
            .await
            .remove(&request.runtime_id);

        Ok(RuntimePauseResult {
            runtime: RuntimeRecord::from_session(&paused),
            checkpoint_id,
            created_at_ms,
            metadata: request.metadata,
            name: request.name,
        })
    }

    async fn resume_runtime_internal(
        &self,
        request: RuntimeResumeRequest,
    ) -> Result<RuntimeResumeResult, SessionServiceError> {
        let mut session = self
            .session_store
            .load(&request.runtime_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: request.runtime_id.clone(),
            })?;
        if session.status == SessionLifecycleStatus::Closed {
            return Err(SessionServiceError::SessionClosed {
                session_id: request.runtime_id,
            });
        }
        if session.status != SessionLifecycleStatus::Paused {
            return Err(SessionServiceError::SessionBusy {
                session_id: request.runtime_id,
                message: "runtime is not paused".to_string(),
            });
        }

        let checkpoint_id = self
            .runtime_checkpoints
            .paused_checkpoint_for_runtime(&request.runtime_id)
            .await
            .ok_or_else(|| SessionServiceError::RuntimeBuild {
                message: format!("paused runtime {} has no checkpoint", request.runtime_id),
            })?;
        let checkpoint = self
            .runtime_checkpoints
            .load(&checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::RuntimeBuild {
                message: format!("paused runtime checkpoint not found: {checkpoint_id}"),
            })?;

        let backend_checkout =
            if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone() {
                Some(
                    self.backend_manager
                        .checkout_backend(BackendCheckoutRequest {
                            checkpoint: backend_checkpoint,
                            backend_id: None,
                            session_id: Some(request.runtime_id.clone()),
                            timeout: None,
                            metadata: request.metadata,
                            resource_limits: Default::default(),
                            options: None,
                            initial_session_status: None,
                        })
                        .await
                        .map_err(|error| {
                            Self::map_backend_error(
                                "failed to resume runtime backend",
                                &request.runtime_id,
                                error,
                            )
                        })?,
                )
            } else {
                None
            };
        let backend_lease = if backend_checkout.is_some() {
            Some(
                self.backend_manager
                    .lease_bound_session(&request.runtime_id)
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!("failed to lease resumed backend: {error}"),
                    })?,
            )
        } else {
            None
        };

        session.status = SessionLifecycleStatus::Idle;
        session.backend_instance = backend_lease.map(|lease| lease.instance());
        session.last_error = None;
        session.updated_at_ms = current_time_ms();
        self.session_store.save(session.clone()).await;
        self.runtime_checkpoints
            .clear_paused_runtime(&request.runtime_id)
            .await;
        self.runtime_checkpoints
            .register_runtime_head(session.session_id.clone(), checkpoint_id)
            .await;
        self.get_or_create_session_handle(session.clone()).await;

        Ok(RuntimeResumeResult {
            runtime: RuntimeRecord::from_session(&session),
        })
    }

    async fn delete_checkpoint_snapshot_internal(
        &self,
        request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteResult, SessionServiceError> {
        let checkpoint = self
            .runtime_checkpoints
            .load(&request.checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: format!("checkpoint:{}", request.checkpoint_id),
            })?;

        let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone() else {
            return Ok(RuntimeCheckpointSnapshotDeleteResult {
                checkpoint_id: checkpoint.checkpoint_id,
                runtime_id: checkpoint.runtime_id,
                provider: None,
                provider_snapshot_id: None,
                provider_snapshot_names: Vec::new(),
                deleted_provider_snapshot: false,
                deleted_at_ms: current_time_ms(),
            });
        };

        let provider = backend_checkpoint.provider.clone();
        let provider_snapshot_id = backend_checkpoint.provider_snapshot_id.clone();
        let provider_snapshot_names = backend_checkpoint.provider_snapshot_names.clone();
        let delete = self
            .backend_manager
            .delete_checkpoint_snapshot(BackendCheckpointSnapshotDeleteRequest {
                checkpoint: backend_checkpoint,
            })
            .await
            .map_err(|error| match error {
                BackendError::UnsupportedBackend { kind } => {
                    SessionServiceError::UnsupportedCapability {
                        capability: format!("delete_checkpoint_snapshot:{kind}"),
                    }
                }
                error => SessionServiceError::RuntimeBuild {
                    message: format!("failed to delete checkpoint backend snapshot: {error}"),
                },
            })?;

        self.runtime_checkpoints
            .clear_backend_snapshot(&request.checkpoint_id)
            .await;

        Ok(RuntimeCheckpointSnapshotDeleteResult {
            checkpoint_id: request.checkpoint_id,
            runtime_id: checkpoint.runtime_id,
            provider: Some(provider),
            provider_snapshot_id,
            provider_snapshot_names,
            deleted_provider_snapshot: delete.deleted,
            deleted_at_ms: current_time_ms(),
        })
    }

    fn build_session_for_turn(
        request: &AppTurnRequest,
        resolved: &ResolvedSessionRuntime,
    ) -> SessionRecord {
        let now_ms = current_time_ms();
        SessionRecord {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            entry: request.entry.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: resolved.descriptor.agent_id.clone(),
                model: resolved.descriptor.model.clone(),
                llm: resolved.descriptor.llm.clone(),
                system_prompt: resolved.descriptor.system_prompt.clone(),
                feature_flags: resolved.descriptor.feature_flags.clone(),
                token_budget: resolved.descriptor.token_budget.clone(),
                workspace_root: resolved.descriptor.workspace_root.clone(),
                max_turns: resolved.descriptor.max_turns,
                tool_manifest: None,
                subagent_roles: resolved.descriptor.subagent_roles.clone(),
                bootstrap_binding: resolved.bootstrap_binding.clone(),
            },
            backend_instance: None,
            paused_backend_checkpoint: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            parent_runtime_id: None,
            forked_from_checkpoint_id: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    fn build_session_for_open(
        request: &SessionOpenRequest,
        resolved: &ResolvedSessionRuntime,
    ) -> SessionRecord {
        let now_ms = current_time_ms();
        SessionRecord {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            entry: request.entry.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: resolved.descriptor.agent_id.clone(),
                model: resolved.descriptor.model.clone(),
                llm: resolved.descriptor.llm.clone(),
                system_prompt: resolved.descriptor.system_prompt.clone(),
                feature_flags: resolved.descriptor.feature_flags.clone(),
                token_budget: resolved.descriptor.token_budget.clone(),
                workspace_root: resolved.descriptor.workspace_root.clone(),
                max_turns: resolved.descriptor.max_turns,
                tool_manifest: None,
                subagent_roles: resolved.descriptor.subagent_roles.clone(),
                bootstrap_binding: resolved.bootstrap_binding.clone(),
            },
            backend_instance: None,
            paused_backend_checkpoint: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            parent_runtime_id: None,
            forked_from_checkpoint_id: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    async fn run_turn_inner(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
        cancellation_token: Option<tokio_util::sync::CancellationToken>,
        tool_event_sink: Option<Arc<dyn agent_contracts::ToolEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        // Stamp request handling BEFORE any await (resolution can take
        // seconds — backend leasing, MCP init): the session actor compares
        // this against a cancel that arrived while no turn was active, so
        // the cancel applies exactly to turns submitted before it (see
        // `SessionActor::idle_cancel_at`).
        let turn_submitted_at = std::time::Instant::now();
        let hooks_enabled = !matches!(
            (
                request.entry.kind.as_ref(),
                request.entry.instance_id.as_deref()
            ),
            (Some(crate::gateway::GatewayEntryKind::Mcp), Some("chatbot"))
        );
        let initialization_lock = self.runtime_initialization_lock(&request.session_id).await;
        let initialization_guard = initialization_lock.lock().await;
        let existing = self.session_store.load(&request.session_id).await;
        let is_new_session = existing.is_none();
        let runtime_input = SessionRuntimeBuildInput::from_turn_request(&request);
        let mut resolved = self
            .runtime_resolver
            .resolve(&runtime_input, existing.as_ref())
            .await?;
        // Preserve a resolver-provided cancel token when this call carries
        // none. The local TUI path (`SessionGateway::spawn_turn` → plain
        // `run_turn`, no cancellation_token argument) wires the TUI's
        // Esc-cancel token into the resolver's static
        // `SessionRuntimeBindings::cancel_token`; unconditionally overwriting
        // it with `None` here severed the TUI from the backend turn — the
        // session actor would fall back to a fresh token the TUI could never
        // fire, so Esc never produced an `AgentOutcome::Cancelled`. An
        // explicit caller-provided `Some(_)` still wins.
        resolved.bindings.cancel_token =
            cancellation_token.or(resolved.bindings.cancel_token.take());
        if let Some(tool_event_sink) = tool_event_sink {
            resolved.bindings.tool_event_sink = Some(tool_event_sink);
        }
        let original_request = request.clone();
        let resolved_agent_role = resolved.descriptor.agent_id.0.clone();
        if let Some(automation) = &self.memory_automation {
            let context = TurnMemoryContext {
                query: request.text.clone(),
                conversation_id: request.conversation_id.clone(),
                message_id: request.message_id.clone(),
                sender_id: request.sender_id.clone(),
                agent_role: resolved_agent_role.clone(),
                timestamp_ms: current_time_ms(),
            };
            match automation.recall(&context).await {
                Ok(memories) if !memories.is_empty() => {
                    let block = render_memory_context(&memories, automation.recall_token_budget());
                    if !block.is_empty() {
                        resolved.descriptor.system_prompt.push_str("\n\n");
                        resolved.descriptor.system_prompt.push_str(&block);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "memory recall degraded; continuing turn")
                }
            }
        }

        let mut seed_session =
            existing.unwrap_or_else(|| Self::build_session_for_turn(&request, &resolved));
        let was_paused = seed_session.status == SessionLifecycleStatus::Paused;
        let backend_lease = lease_session_backend(
            self.backend_manager.as_ref(),
            &seed_session,
            &resolved,
            self.session_store.clone(),
        )
        .await?;
        if let Err(error) =
            crate::gateway::finalize_e2b_runtime(&mut resolved, backend_lease.backend()).await
        {
            if is_new_session {
                self.backend_manager
                    .release_session(&request.session_id)
                    .await
                    .ok();
            }
            return Err(error);
        }
        seed_session.runtime.system_prompt = resolved.descriptor.system_prompt.clone();
        seed_session.runtime.workspace_root = resolved.descriptor.workspace_root.clone();
        seed_session.runtime.bootstrap_binding = resolved.bootstrap_binding.clone();
        let backend_updated = sync_session_backend_instance(&mut seed_session, &backend_lease);
        if was_paused {
            seed_session.status = SessionLifecycleStatus::Idle;
            seed_session.paused_backend_checkpoint = None;
            seed_session.last_error = None;
        }
        if backend_updated || was_paused {
            seed_session.updated_at_ms = current_time_ms();
        }
        if is_new_session || backend_updated || was_paused || resolved.bootstrap_binding.is_some() {
            self.session_store.save(seed_session.clone()).await;
        }
        drop(initialization_guard);

        if is_new_session && hooks_enabled {
            let hook_point =
                session_lifecycle_hook_point(&resolved.descriptor.agent_id.0, "created");
            self.fire_session_hooks(
                HookInvokeInput::SessionCreated {
                    input: SessionCreatedHookInput {
                        session_id: seed_session.session_id.clone(),
                        sender_id: seed_session.sender_id.clone(),
                        workspace: session_workspace_for_hook(&seed_session.runtime.workspace_root),
                    },
                    metadata: HookInvokeMetadata::default(),
                },
                hook_point,
            )
            .await;
        }

        let lifecycle_session_id = request.session_id.clone();
        let lifecycle_sender_id = request.sender_id.clone();
        let lifecycle_agent_id = resolved.descriptor.agent_id.0.clone();
        let lifecycle_workspace = session_workspace_for_hook(&seed_session.runtime.workspace_root);
        let lifecycle_chain_depth = request.chain_depth;

        let prior_memory_context = seed_session
            .loop_state
            .as_ref()
            .map(|loop_state| loop_state.messages.clone())
            .unwrap_or_default();
        let handle = self.get_or_create_session_handle(seed_session).await;

        let mut turn_result = handle
            .run_turn(
                request,
                resolved,
                event_sink,
                interaction_handle,
                channel_file_sender,
                turn_submitted_at,
            )
            .await;

        // Dispatch `*.Session.lifecycle.state`: `Ok` → `state="idle"`
        // (awaited; actions collected); `Err` → `state="failed"`
        // (fire-and-forget; actions discarded, error logged).
        if hooks_enabled {
            match turn_result.as_mut() {
                Ok(turn) => {
                    let actions = self
                        .fire_session_state_hook_and_collect_actions(
                            SessionStateHookEvent {
                                session_id: lifecycle_session_id,
                                sender_id: lifecycle_sender_id,
                                agent_id: lifecycle_agent_id,
                                state: SessionLifecycleStatus::Idle.as_tag().to_string(),
                                outcome: turn.outcome.as_tag().to_string(),
                                workspace: lifecycle_workspace,
                            },
                            lifecycle_chain_depth,
                        )
                        .await;
                    turn.hook_actions = actions;
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        session_id = %lifecycle_session_id,
                        agent_id = %lifecycle_agent_id,
                        "turn failed; dispatching state=\"failed\" session lifecycle hook"
                    );
                    self.fire_session_state_hook_background(SessionStateHookEvent {
                        session_id: lifecycle_session_id,
                        sender_id: lifecycle_sender_id,
                        agent_id: lifecycle_agent_id,
                        state: SessionLifecycleStatus::Failed.as_tag().to_string(),
                        outcome: SessionStateOutcome::Error.as_tag().to_string(),
                        workspace: lifecycle_workspace,
                    });
                }
            }
        }

        if let (Some(automation), Ok(turn)) = (&self.memory_automation, &turn_result) {
            let ingest = CompletedTurnIngest {
                message_id: original_request.message_id.clone().unwrap_or_else(|| {
                    format!("{}:{}", original_request.session_id, current_time_ms())
                }),
                conversation_id: original_request.conversation_id.clone(),
                sender_id: original_request.sender_id.clone(),
                agent_role: resolved_agent_role,
                timestamp_ms: current_time_ms(),
                user_text: original_request.text.clone(),
                assistant_text: turn.visible_reply.clone(),
                recent_messages: recent_memory_context_messages(
                    &prior_memory_context,
                    automation.context_messages(),
                ),
                retries: 0,
                next_attempt_ms: 0,
                failed: false,
            };
            if let Err(error) = automation.enqueue_ingest(ingest).await {
                tracing::warn!(error = %error, "memory ingest degraded; completed turn preserved");
            }
        }

        turn_result
    }

    /// Body of [`SessionControlPlane::open_session`] after the lease has been
    /// acquired. Contains the original open / resume + state-preservation
    /// logic. The trait impl wraps this with lease acquire + rollback.
    async fn open_session_inner(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionRecord, SessionServiceError> {
        let initialization_lock = self.runtime_initialization_lock(&request.session_id).await;
        let _initialization_guard = initialization_lock.lock().await;
        let existing_record = self.session_store.load(&request.session_id).await;
        let is_new_session = existing_record.is_none();
        let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
        // Resolve before every fast return so E2B binding conflicts cannot be
        // bypassed by an existing handle or a paused runtime.
        let mut resolved = self
            .runtime_resolver
            .resolve(&runtime_input, existing_record.as_ref())
            .await?;
        if let Some(existing) = &existing_record {
            if existing.status == SessionLifecycleStatus::Paused {
                return Ok(existing.clone());
            }
        }
        // Reuse an already-running in-memory handle if present. We
        // intentionally do NOT fall back to the store here: doing so via
        // `handle_for_session` would silently create a handle from a
        // stale/imported record (e.g. from `/load`) without leasing a
        // backend or running the state-preservation logic below, which
        // loses the LLM's context on the next turn. Records without a live
        // handle fall through to the build path, which properly preserves
        // imported state and leases a backend.
        if let Some(handle) = self
            .sessions_handler
            .lock()
            .await
            .get(&request.session_id)
            .cloned()
        {
            return handle.snapshot().await;
        }

        let mut session = Self::build_session_for_open(&request, &resolved);
        // Preserve state from any pre-existing store record (e.g. imported
        // via `/load`). Without this, `build_session_for_open` would
        // initialise `loop_state`/`memory_snapshot` to `None` and the
        // subsequent `session_store.save` below would overwrite the imported
        // record — silently erasing the LLM's message history and leaving
        // the model with no prior context even though the TUI still echoes
        // the old chat messages.
        if let Some(existing) = existing_record {
            session.loop_state = existing.loop_state;
            session.memory_snapshot = existing.memory_snapshot;
            session.agents = existing.agents;
            session.subagent_state = existing.subagent_state;
            session.parent_runtime_id = existing.parent_runtime_id;
            session.forked_from_checkpoint_id = existing.forked_from_checkpoint_id;
            session.created_at_ms = existing.created_at_ms;
        }
        let backend_lease = lease_session_backend(
            self.backend_manager.as_ref(),
            &session,
            &resolved,
            self.session_store.clone(),
        )
        .await?;
        if let Err(error) =
            crate::gateway::finalize_e2b_runtime(&mut resolved, backend_lease.backend()).await
        {
            if is_new_session {
                self.backend_manager
                    .release_session(&request.session_id)
                    .await
                    .ok();
            }
            return Err(error);
        }
        session.runtime.system_prompt = resolved.descriptor.system_prompt.clone();
        session.runtime.workspace_root = resolved.descriptor.workspace_root.clone();
        session.runtime.bootstrap_binding = resolved.bootstrap_binding.clone();
        if sync_session_backend_instance(&mut session, &backend_lease) {
            session.updated_at_ms = current_time_ms();
        }
        self.session_store.save(session.clone()).await;

        let hook_point = session_lifecycle_hook_point(&resolved.descriptor.agent_id.0, "created");
        self.fire_session_hooks(
            HookInvokeInput::SessionCreated {
                input: SessionCreatedHookInput {
                    session_id: session.session_id.clone(),
                    sender_id: session.sender_id.clone(),
                    workspace: session_workspace_for_hook(&session.runtime.workspace_root),
                },
                metadata: HookInvokeMetadata::default(),
            },
            hook_point,
        )
        .await;

        self.get_or_create_session_handle(session)
            .await
            .snapshot()
            .await
    }

    /// Body of [`SessionControlPlane::force_close_session`] and
    /// [`SessionControlPlane::force_close_session_with_lease`]: cascade-close
    /// children, mark this session Closed, release backend, delete checkpoint
    /// snapshots. The lease-checked variant wraps this with a holder check
    /// (skipped when the caller is a daemon-internal principal) and a lease
    /// cleanup at the end.
    async fn force_close_session_inner(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        // 1. Cascade-close children first (bottom-up) so descendant sandboxes
        //    and snapshots are reclaimed together with the parent. A child
        //    failure does not abort the parent's close.
        self.cascade_close_children(session_id).await;

        // 2. Close this session's handle (mark Closed) or fall back to the
        //    store-only path when no live handle exists.
        let (closed, was_already_closed) = self.close_self_record(session_id).await?;

        // 3. Release backends leased for this session.
        if let Err(error) = self.backend_manager.release_session(session_id).await {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to release session backends"
            );
        }

        // 4. Delete provider snapshots + drop in-memory checkpoint records.
        self.cleanup_runtime_checkpoints(session_id).await;

        // 5. Fire the `Session.lifecycle.closed` hook (only if the session
        //    was not already Closed — avoids double-firing on idempotent
        //    closes).
        if !was_already_closed {
            let hook_point = session_lifecycle_hook_point(&closed.runtime.agent_id.0, "closed");
            self.fire_session_hooks(
                HookInvokeInput::SessionClosed {
                    input: SessionClosedHookInput {
                        session_id: closed.session_id.clone(),
                        sender_id: closed.sender_id.clone(),
                        workspace: session_workspace_for_hook(&closed.runtime.workspace_root),
                    },
                    metadata: HookInvokeMetadata::default(),
                },
                hook_point,
            )
            .await;
        }

        // 6. Drop the store record + in-memory handle + attach lease so a
        //    future `open_session` for the same id starts fresh.
        self.evict_session_artifacts(session_id).await;

        Ok(closed)
    }

    /// Step 1 of [`force_close_session_inner`]: collect direct children
    /// (sessions forked via `checkout` whose `parent_runtime_id` equals this
    /// session) and recursively close each one. A child failure is logged but
    /// does not abort the parent's close — a single bad child must not strand
    /// the whole subtree.
    ///
    /// The recursive call goes through the trait method
    /// `SessionControlPlane::force_close_session` (rather than
    /// `force_close_session_inner` directly) so `#[async_trait]` boxes the
    /// futures, providing the indirection `async fn` recursion needs.
    /// Children never get a lease check (the parent's lease covers the
    /// subtree).
    async fn cascade_close_children(&self, session_id: &str) {
        let children = self.session_store.list_children(session_id).await;
        for child in &children {
            if let Err(error) = self.force_close_session(&child.session_id).await {
                tracing::warn!(
                    session_id = %child.session_id,
                    parent_session_id = %session_id,
                    error = %error,
                    "cascade close of child runtime failed; continuing with parent close"
                );
            }
        }
    }

    /// Shared tail of [`force_close_session_with_lease`] (the trait close
    /// path), its anonymous-caller sub-path, and the reaper's
    /// [`reap_one_record`]: run `force_close_session_inner`, and on error
    /// evict residual artifacts so the session is not bricked. On success
    /// `force_close_session_inner` itself evicts everything as step 6, so
    /// this helper does nothing extra. `evict_session_artifacts` is
    /// idempotent on absent keys, so partial step-6 progress is covered.
    async fn close_and_evict_on_err(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        let result = self.force_close_session_inner(session_id).await;
        if result.is_err() {
            // `force_close_session_inner` bailed before reaching step 6
            // (`evict_session_artifacts`). Without this cleanup a handle
            // stuck at phase `Closing` would reject every subsequent
            // `run_turn` forever, and a surviving store record would be
            // re-selected on every re-open — the session is bricked until
            // the 2 h reaper happens to clean it up. Evict everything here
            // so the next `open_session` starts fresh. Called from both the
            // trait close path and the reaper's `reap_one_record`.
            self.evict_session_artifacts(session_id).await;
        }
        result
    }

    /// Step 2 of [`force_close_session_inner`]: close this session's handle
    /// (mark Closed) via the actor, or fall back to the store-only path when
    /// no live handle exists. Returns the closed `SessionRecord` and whether
    /// it was already `Closed` (used to gate the `SessionClosed` hook).
    async fn close_self_record(
        &self,
        session_id: &str,
    ) -> Result<(SessionRecord, bool), SessionServiceError> {
        if let Some(handle) = self.handle_for_session(session_id).await {
            let before = handle.snapshot().await?;
            let was_already_closed = before.status == SessionLifecycleStatus::Closed;
            Ok((handle.force_close().await?, was_already_closed))
        } else {
            let Some(mut existing) = self.session_store.load(session_id).await else {
                return Err(SessionServiceError::SessionNotFound {
                    session_id: session_id.to_string(),
                });
            };
            let was_already_closed = existing.status == SessionLifecycleStatus::Closed;
            existing.status = SessionLifecycleStatus::Closed;
            existing.updated_at_ms = current_time_ms();
            self.session_store.save(existing.clone()).await;
            Ok((existing, was_already_closed))
        }
    }

    /// Step 4 of [`force_close_session_inner`]: delete provider snapshots
    /// (e.g. e2b snapshots) for every checkpoint created by this runtime,
    /// then drop the in-memory checkpoint records. Failures are logged but
    /// do not abort the close — local tracking is still cleared so it does
    /// not accumulate.
    async fn cleanup_runtime_checkpoints(&self, session_id: &str) {
        let checkpoints = self
            .runtime_checkpoints
            .list_checkpoints_for_runtime(session_id)
            .await;
        for checkpoint in &checkpoints {
            if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.as_ref() {
                if backend_checkpoint.provider_snapshot_id.is_some() {
                    if let Err(error) = self
                        .backend_manager
                        .delete_checkpoint_snapshot(BackendCheckpointSnapshotDeleteRequest {
                            checkpoint: backend_checkpoint.clone(),
                        })
                        .await
                    {
                        tracing::warn!(
                            checkpoint_id = %checkpoint.checkpoint_id,
                            session_id = %session_id,
                            error = %error,
                            "failed to delete checkpoint snapshot during close; snapshot may linger remotely"
                        );
                    }
                }
            }
        }
        self.runtime_checkpoints.remove_runtime(session_id).await;
    }

    /// Step 6 of [`force_close_session_inner`]: drop the store record (so
    /// subsequent `load` / `resume_session` calls report the session as gone),
    /// evict the in-memory handle, and remove the attach lease. Covers the
    /// cascade-close path: children closed recursively via
    /// `force_close_session` → `force_close_session_inner` would otherwise
    /// leave each child's lease (held by a different `client_id`) surviving
    /// the close. `HashMap::remove` on an absent key is a no-op, so callers
    /// that already removed the lease (`force_close_session_with_lease`, the
    /// reaper) are unaffected.
    async fn evict_session_artifacts(&self, session_id: &str) {
        self.session_store.delete(session_id).await;
        self.sessions_handler.lock().await.remove(session_id);
        self.sessions_lease.remove(session_id).await;
    }
}

#[async_trait]
impl SessionService for CoreBackedSessionService {
    async fn run_turn(
        &self,
        request: AppTurnRequest,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, None, None, None, None, None)
            .await
    }

    async fn run_turn_with_events(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, event_sink, None, None, None, None)
            .await
    }

    async fn run_turn_with_interaction(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
        cancellation_token: Option<tokio_util::sync::CancellationToken>,
        tool_event_sink: Option<Arc<dyn agent_contracts::ToolEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(
            request,
            event_sink,
            interaction_handle,
            channel_file_sender,
            cancellation_token,
            tool_event_sink,
        )
        .await
    }

    async fn export_session(&self, session_id: &str) -> Result<SessionRecord, SessionServiceError> {
        match self.session_store.load(session_id).await {
            Some(record) => Ok(record),
            None => Err(SessionServiceError::SessionNotFound {
                session_id: session_id.to_string(),
            }),
        }
    }
}

#[async_trait]
impl SessionControlPlane for CoreBackedSessionService {
    async fn list_sandboxes(&self) -> Result<Vec<BackendInfo>, SessionServiceError> {
        Ok(self
            .backend_manager
            .list_backends(BackendListFilter::default())
            .await)
    }

    async fn hibernate_idle_session(
        &self,
        session_id: &str,
        idle_before_ms: u64,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        let Some(handle) = self.sessions_handler.lock().await.get(session_id).cloned() else {
            return Ok(None);
        };
        let Some(paused) = handle.hibernate_idle(idle_before_ms).await? else {
            return Ok(None);
        };
        self.sessions_handler.lock().await.remove(session_id);
        self.backend_manager
            .release_session(session_id)
            .await
            .map_err(|error| SessionServiceError::RuntimeShutdown {
                message: format!("failed to release hibernated local backend: {error}"),
            })?;
        Ok(Some(paused))
    }

    async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionRecord, SessionServiceError> {
        // Acquire the attach lease BEFORE touching the session store / handle
        // — atomic under the lease table's mutex, so concurrent `open_session`
        // calls for the same session_id serialise here.
        //
        // Anonymous callers (`None` / empty `client_id`) bypass the acquire
        // (useful for tests / curl probes / gradual rollout). When
        // `enforce_anonymous_lease` is on (`XIAOO_ENFORCE_LEASE=on`) they are
        // rejected with `LeaseRequired` here, mirroring `assert_lease_holder` /
        // `force_close_session_with_lease` so `open` is not the one mutating
        // RPC that defeats strict single-writer. Daemon-internal principals
        // (`daemon:`) also bypass the acquire (cooperative background callers
        // that would otherwise steal the holder's lease); the bypass is
        // explicit and logged.
        let Some(client_id) = request.client_id.as_deref().filter(|s| !s.is_empty()) else {
            // Anonymous caller: reject under strict enforcement, else skip the
            // lease acquire and open directly (no lease to roll back on
            // failure).
            if self.anonymous_lease_enforced() {
                return Err(SessionServiceError::LeaseRequired {
                    session_id: request.session_id.clone(),
                });
            }
            return self.open_session_inner(request).await;
        };

        if is_daemon_principal(client_id) {
            tracing::debug!(
                session_id = %request.session_id,
                client_id = %client_id,
                "open_session: daemon-internal principal bypasses lease acquire"
            );
        } else {
            match self
                .sessions_lease
                .acquire(
                    &request.session_id,
                    client_id,
                    request.client_pid,
                    request.client_hostname.clone(),
                )
                .await
            {
                LeaseAcquireOutcome::Acquired => {}
                LeaseAcquireOutcome::ClockSkew => {
                    return Err(SessionServiceError::LeaseClockSkew {
                        session_id: request.session_id.clone(),
                    });
                }
                LeaseAcquireOutcome::Busy {
                    holder_client_id,
                    holder_hostname,
                    holder_pid,
                    last_heartbeat_ms,
                    stale,
                } => {
                    return Err(SessionServiceError::SessionAttachedByAnotherClient {
                        session_id: request.session_id,
                        holder_client_id,
                        holder_hostname: holder_hostname.unwrap_or_default(),
                        holder_pid: holder_pid.unwrap_or(0),
                        last_heartbeat_ms,
                        stale,
                    });
                }
            }
        }

        // Open / resume the session record. Any failure on this path rolls
        // back the lease so a transient resolver / backend error doesn't leave
        // a phantom lease blocking subsequent retries. `detach` only removes
        // the lease when `lease.client_id == client_id`, so it acts as a CAS:
        // a lease that was taken over by another client (via the stale
        // path) while we were building the session is left untouched (the
        // new holder wins).
        match self.open_session_inner(request.clone()).await {
            Ok(record) => Ok(record),
            Err(error) => {
                self.sessions_lease
                    .detach(&request.session_id, client_id)
                    .await;
                Err(error)
            }
        }
    }

    async fn resume_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        match self.handle_for_session(session_id).await {
            Some(handle) => Ok(Some(handle.snapshot().await?)),
            None => Ok(None),
        }
    }

    async fn force_close_session(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        self.force_close_session_inner(session_id).await
    }

    async fn force_close_session_with_lease(
        &self,
        session_id: &str,
        client_id: Option<&str>,
    ) -> Result<SessionRecord, SessionServiceError> {
        // Single entry-point for the close path. Each "allowed to close"
        // branch falls through to the same `close_and_evict_on_err` tail so
        // the error-cleanup contract lives in one place.
        //
        // Mirror `assert_lease_holder`'s anonymous-caller policy: when
        // `enforce_anonymous_lease` is on, reject anonymous callers with
        // `LeaseRequired` so the close path can't bypass
        // `XIAOO_ENFORCE_LEASE`.
        let Some(client_id) = client_id.filter(|s| !s.is_empty()) else {
            if self.anonymous_lease_enforced() {
                return Err(SessionServiceError::LeaseRequired {
                    session_id: session_id.to_string(),
                });
            }
            // Anonymous callers allowed under gradual rollout; fall through
            // to the close without a holder check.
            return self.close_and_evict_on_err(session_id).await;
        };
        // Daemon-internal principals bypass the holder check; the close path
        // still drops the lease below so a phantom entry doesn't outlive the
        // session.
        if is_daemon_principal(client_id) {
            tracing::debug!(
                session_id = %session_id,
                client_id = %client_id,
                "force_close_session_with_lease: daemon-internal principal bypasses holder check"
            );
        } else {
            // Read-only holder check (`check_holder`): the close path
            // removes the lease unconditionally below, so we don't want the
            // stale-takeover side effect of `check_or_takeover_holder`
            // rewriting the lease just before we drop it.
            if let Err(failure) = self
                .sessions_lease
                .check_holder(session_id, Some(client_id))
                .await
            {
                return Err(SessionServiceError::from_lease_check_failure(
                    session_id, failure,
                ));
            }
        }
        self.close_and_evict_on_err(session_id).await
    }

    async fn heartbeat_session(
        &self,
        request: SessionHeartbeatRequest,
    ) -> Result<(), SessionServiceError> {
        let Some(client_id) = request.client_id.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(());
        };
        // `sessions_lease.heartbeat` returns a `LeaseCheckFailure` whose
        // `stale` field was computed under the table mutex at the moment of
        // rejection — no re-computation here, so the TUI sees the same
        // staleness verdict the daemon used to reject the heartbeat.
        // `client_pid` / `client_hostname` are only consumed on the
        // auto-re-acquire path (daemon restart wiped the table), so holder
        // identity is preserved instead of silently dropping to `None`.
        match self
            .sessions_lease
            .heartbeat(
                &request.session_id,
                client_id,
                request.client_pid,
                request.client_hostname,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(failure) => Err(SessionServiceError::from_lease_check_failure(
                request.session_id,
                failure,
            )),
        }
    }

    async fn detach_session(
        &self,
        request: SessionDetachRequest,
    ) -> Result<(), SessionServiceError> {
        if let Some(client_id) = request.client_id.as_deref().filter(|s| !s.is_empty()) {
            self.sessions_lease
                .detach(&request.session_id, client_id)
                .await;
        }
        Ok(())
    }

    async fn assert_lease_holder(
        &self,
        session_id: &str,
        client_id: Option<&str>,
    ) -> Result<(), SessionServiceError> {
        // Anonymous callers bypass the lease check by default (gradual
        // rollout). When `enforce_anonymous_lease` is on (via
        // `XIAOO_ENFORCE_LEASE`), reject with `LeaseRequired`.
        let Some(client_id) = client_id.filter(|s| !s.is_empty()) else {
            return if self.anonymous_lease_enforced() {
                Err(SessionServiceError::LeaseRequired {
                    session_id: session_id.to_string(),
                })
            } else {
                Ok(())
            };
        };
        // Daemon-internal principals bypass the holder check explicitly; the
        // bypass is logged so audits can distinguish it from the legacy
        // anonymous bypass.
        if is_daemon_principal(client_id) {
            tracing::debug!(
                session_id = %session_id,
                client_id = %client_id,
                "assert_lease_holder: daemon-internal principal bypasses holder check"
            );
            return Ok(());
        }
        match self
            .sessions_lease
            .check_or_takeover_holder(session_id, Some(client_id))
            .await
        {
            Ok(()) => Ok(()),
            Err(failure) => Err(SessionServiceError::from_lease_check_failure(
                session_id, failure,
            )),
        }
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeRecord>, SessionServiceError> {
        let mut runtimes = self
            .session_store
            .list_all()
            .await
            .iter()
            .map(RuntimeRecord::from_session)
            .collect::<Vec<_>>();
        runtimes.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.runtime_id.cmp(&right.runtime_id))
        });
        Ok(runtimes)
    }

    async fn list_runtime_checkpoints(
        &self,
    ) -> Result<Vec<crate::RuntimeCheckpointSummary>, SessionServiceError> {
        Ok(self.runtime_checkpoints.list_checkpoint_summaries().await)
    }

    async fn checkpoint_runtime(
        &self,
        request: RuntimeCheckpointRequest,
    ) -> Result<RuntimeCheckpointResult, SessionServiceError> {
        self.checkpoint_runtime_internal(request).await
    }

    async fn checkout_runtime(
        &self,
        request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutResult, SessionServiceError> {
        self.checkout_runtime_internal(request).await
    }

    async fn pause_runtime(
        &self,
        request: RuntimePauseRequest,
    ) -> Result<RuntimePauseResult, SessionServiceError> {
        self.pause_runtime_internal(request).await
    }

    async fn resume_runtime(
        &self,
        request: RuntimeResumeRequest,
    ) -> Result<RuntimeResumeResult, SessionServiceError> {
        self.resume_runtime_internal(request).await
    }

    async fn delete_checkpoint_snapshot(
        &self,
        request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteResult, SessionServiceError> {
        self.delete_checkpoint_snapshot_internal(request).await
    }

    async fn exec_runtime(
        &self,
        request: RuntimeExecRequest,
    ) -> Result<RuntimeExecResult, SessionServiceError> {
        let lease = self
            .lease_bound_backend_for_idle_runtime(&request.runtime_id)
            .await?;
        let env = (!request.env.is_empty()).then(|| request.env.into_iter().collect());
        let backend = lease.backend();
        let shell = resolve_runtime_exec_shell(request.shell, backend.exec().default_shell());
        let result = backend
            .exec()
            .exec(ExecRequest {
                command: request.command,
                args: Vec::new(),
                shell: Some(shell),
                cwd: request.cwd.map(BackendPath::from_raw),
                timeout_ms: request.timeout_ms,
                env,
                ..Default::default()
            })
            .await;
        let result = match result {
            Ok(result) => result,
            Err(agent_contracts::backend::OperationError::ExecutionInterrupted {
                message,
                stdout,
                stderr,
                state,
            }) => {
                return Err(SessionServiceError::RuntimeExecInterrupted {
                    message: format!("runtime exec failed: {message}"),
                    stdout_base64: base64::engine::general_purpose::STANDARD.encode(stdout),
                    stderr_base64: base64::engine::general_purpose::STANDARD.encode(stderr),
                    execution_state: state,
                });
            }
            Err(error) => {
                return Err(SessionServiceError::CoreRun {
                    message: format!("runtime exec failed: {error}"),
                });
            }
        };

        Ok(RuntimeExecResult {
            stdout_base64: base64::engine::general_purpose::STANDARD.encode(result.stdout),
            stderr_base64: base64::engine::general_purpose::STANDARD.encode(result.stderr),
            exit_code: result.exit_code,
            timed_out: result.timed_out,
        })
    }

    async fn read_runtime_file(
        &self,
        request: RuntimeReadFileRequest,
    ) -> Result<RuntimeReadFileResult, SessionServiceError> {
        let lease = self
            .lease_bound_backend_for_idle_runtime(&request.runtime_id)
            .await?;
        let content = lease
            .backend()
            .files()
            .read_bytes(ReadBytesRequest {
                path: BackendPath::from_raw(request.path),
            })
            .await
            .map_err(|error| SessionServiceError::CoreRun {
                message: format!("runtime file read failed: {error}"),
            })?;

        Ok(RuntimeReadFileResult {
            content_base64: base64::engine::general_purpose::STANDARD.encode(content),
        })
    }

    async fn write_runtime_file(
        &self,
        request: RuntimeWriteFileRequest,
    ) -> Result<RuntimeWriteFileResult, SessionServiceError> {
        let lease = self
            .lease_bound_backend_for_idle_runtime(&request.runtime_id)
            .await?;
        let content = base64::engine::general_purpose::STANDARD
            .decode(request.content_base64)
            .map_err(|error| SessionServiceError::RuntimeBuild {
                message: format!("invalid content_base64: {error}"),
            })?;
        let outcome = lease
            .backend()
            .files()
            .write_bytes(WriteBytesRequest {
                path: BackendPath::from_raw(request.path),
                content,
                mode: WriteMode::Overwrite,
            })
            .await
            .map_err(|error| SessionServiceError::CoreRun {
                message: format!("runtime file write failed: {error}"),
            })?;

        Ok(RuntimeWriteFileResult {
            path: outcome.path.native().to_string(),
            created: outcome.created,
        })
    }

    async fn submit_input(
        &self,
        session_id: &str,
        input: SessionInput,
    ) -> Result<crate::gateway::SessionSubmitReceipt, SessionServiceError> {
        match input {
            SessionInput::CancelActiveTurn => {
                let Some(handle) = self.handle_for_session(session_id).await else {
                    return Err(SessionServiceError::SessionNotFound {
                        session_id: session_id.to_string(),
                    });
                };
                handle.cancel_active_turn().await
            }
            SessionInput::Turn { .. } => Err(SessionServiceError::UnsupportedCapability {
                capability: "submit_input.turn".to_string(),
            }),
            SessionInput::Interaction { .. } => Err(SessionServiceError::UnsupportedCapability {
                capability: "submit_input.interaction".to_string(),
            }),
            SessionInput::InputChunk { .. } => Err(SessionServiceError::UnsupportedCapability {
                capability: "submit_input.input_chunk".to_string(),
            }),
        }
    }
}

#[async_trait]
impl SubagentControl for CoreBackedSessionService {
    async fn spawn(
        &self,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError> {
        let Some(handle) = self.handle_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        let supervisor = handle.supervisor();
        supervisor.spawn_subagent(request).await
    }

    async fn join(
        &self,
        request: JoinSubagentRequest,
    ) -> Result<JoinSubagentResult, SubagentControlError> {
        let Some(handle) = self.handle_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        let supervisor = handle.supervisor();
        supervisor.join_subagent(request).await
    }
}

impl From<SessionRuntimeResolveError> for SessionServiceError {
    fn from(value: SessionRuntimeResolveError) -> Self {
        match value {
            SessionRuntimeResolveError::InvalidBootstrap { message } => {
                Self::InvalidRequest { message }
            }
            SessionRuntimeResolveError::BootstrapConflict { message } => {
                Self::RuntimeConflict { message }
            }
            SessionRuntimeResolveError::BootstrapTooLarge { message } => {
                Self::PayloadTooLarge { message }
            }
            other => Self::RuntimeResolve {
                message: other.to_string(),
            },
        }
    }
}

impl From<SessionStoreError> for SessionServiceError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore {
            message: value.to_string(),
        }
    }
}

/// Wall time in ms for non-lease session timestamps (`updated_at_ms`,
/// `created_at_ms`, `deleted_at_ms`, `completed_at_ms`).
///
/// **Fail-OPEN**: returns `0` when the wall clock is before `UNIX_EPOCH`
/// (broken RTC / pre-NTP boot). Acceptable for these timestamps because the
/// lease acquire uses the fail-CLOSED
/// [`crate::gateway::session_lease::current_time_ms`] (which rejects on clock
/// skew) — by the time we stamp `created_at_ms` here the clock is already
/// known-good, and the reaper skips `Closed` records so a `0` timestamp
/// never causes a false reap.
///
/// Delegates to the canonical fail-closed
/// [`crate::gateway::session_lease::current_time_ms`] for the actual clock
/// read (single source of truth); the only difference is the error policy —
/// this wrapper logs a `warn!` and returns `0` (non-fatal), the lease code
/// bubbles `Err(ClockSkew)` to the caller. The lease table MUST NOT use this
/// helper (single-writer enforcement must degrade to "reject all acquires",
/// not "treat every lease as fresh").
fn current_time_ms() -> u64 {
    crate::gateway::session_lease::current_time_ms().unwrap_or_else(|error| {
        tracing::warn!(
            error = %error,
            "wall clock is before UNIX_EPOCH while stamping a non-lease session \
             timestamp; using 0 (lease enforcement itself stays fail-closed)"
        );
        0
    })
}

fn recent_memory_context_messages(
    messages: &[agent_types::ChatMessage],
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut recent = messages
        .iter()
        .rev()
        .filter_map(|message| {
            let text = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    agent_types::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        })
        .take(limit)
        .collect::<Vec<_>>();
    recent.reverse();
    recent
}

/// Build a `*.Session.lifecycle.<stage>` hook point id. Consolidates the
/// inline `format!("{}.Session.lifecycle.<stage>", agent_id)` previously
/// repeated across the session created/closed/state call sites.
fn session_lifecycle_hook_point(agent_id: &str, stage: &str) -> HookPointId {
    HookPointId(format!("{}.Session.lifecycle.{}", agent_id, stage))
}

/// Stamp each `SendPrompt` action with `chain_depth = emitting_turn_depth
/// + 1` (overwriting any plugin-supplied value) and drop it once the stamped
/// value **reaches** `max_depth` (i.e., `next_depth >= max_depth` — exclusive
/// upper bound). Other action kinds pass through unchanged. Pure (no I/O).
///
/// Semantics: `max_depth = N` permits N turns total — the user-initiated
/// turn (depth `0`) plus `N - 1` `send_prompt`-triggered turns (depths
/// `1..=N-1`). The cap defaults to `DEFAULT_MAX_PROMPT_CHAIN_DEPTH` (128),
/// configurable via `[hooker].max_prompt_chain_depth`. The stamped
/// `chain_depth` rides along on the forwarded action so the TUI can relay
/// it back via `RuntimeTurnRequest.chain_depth`, letting the daemon track
/// the resulting turn's depth and re-enforce the cap when that turn ends.
fn stamp_and_cap_send_prompt_actions(
    actions: Vec<HookAction>,
    emitting_turn_depth: usize,
    max_depth: usize,
) -> Vec<HookAction> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            HookAction::SendPrompt {
                session_id, text, ..
            } => {
                let next_depth = emitting_turn_depth.saturating_add(1);
                if next_depth >= max_depth {
                    tracing::warn!(
                        session_id = %session_id,
                        next_depth,
                        max_depth,
                        "send_prompt hook action dropped: chain depth reaches cap"
                    );
                    return None;
                }
                Some(HookAction::SendPrompt {
                    session_id,
                    text,
                    chain_depth: next_depth,
                })
            }
            other => Some(other),
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../../tests/unit/shared/gateway/session_service_impl_test.rs"]
mod tests;
