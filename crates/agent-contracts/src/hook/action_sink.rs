use agent_types::hook::HookAction;
use async_trait::async_trait;

/// Maximum number of actions a single hook response may request.
///
/// The host enforces this ceiling to prevent runaway batches and recursive
/// `hook → action → hook` loops. Batches longer than this are truncated (the
/// trailing actions are logged and dropped). The value is intentionally small
/// so a misbehaving plugin cannot flood the daemon with session actions.
pub const MAX_ACTION_DEPTH: usize = 3;

/// Sink for side-effect actions requested by plugin hookers alongside their
/// primary hook result.
///
/// When a plugin returns `{"result":"ack","actions":[...]}`, the host
/// (daemon in remote mode, TUI process in local mode) executes the actions
/// through this trait. The sink is the bridge between the hook adaptor
/// (which only parses JSON) and the runtime that owns session state.
///
/// ## Execution semantics
///
/// - **Daemon-side execution first**: in remote mode the daemon runs each
///   action against its own `SessionControlPlane` (e.g. `open_session` for
///   `CreateSession`/`SwitchSession`) and then returns the surviving actions
///   so the caller can forward them to the TUI via the SSE `Done` event.
///   Actions that failed daemon-side execution are filtered out.
/// - **Depth limiting**: the host enforces [`MAX_ACTION_DEPTH`] (3) to
///   prevent recursive `hook→action→hook` loops; batches longer than the
///   limit are truncated (extras logged and dropped).
#[async_trait]
pub trait HookActionSink: Send + Sync {
    /// Execute a batch of actions on the daemon side. Returns the subset of
    /// actions that should be forwarded to the TUI (actions that failed
    /// daemon-side execution are filtered out).
    ///
    /// Implementations should enforce [`MAX_ACTION_DEPTH`]: if `actions` is
    /// longer than the limit, only the first [`MAX_ACTION_DEPTH`] entries are
    /// processed and the rest are dropped (with a `tracing::warn!`).
    async fn execute_on_daemon(&self, actions: Vec<HookAction>) -> Vec<HookAction>;
}
