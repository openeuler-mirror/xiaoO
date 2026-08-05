use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

#[async_trait]
pub trait InteractionHandle: Send + Sync {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse;

    /// Whether [`ask`](Self::ask) enforces its own timeout and cleanup
    /// (e.g. `ChannelInteractionHandle`, which rounds the configured
    /// timeout up to whole minutes, sends the user a "timed out" notice,
    /// and removes its `PendingInteraction` entry on expiry).
    ///
    /// When this returns `true`, the gateway skips its defensive outer
    /// `tokio::select!` timeout around `ask` so the inner future is never
    /// dropped mid-await (which would bypass the inner cleanup branch and
    /// leak the underlying pending entry). Handles that may block
    /// indefinitely (e.g. `RemoteSseInteractionHandle`, which only awaits
    /// a `oneshot::Receiver`) return the default `false` so the gateway's
    /// outer timeout caps them.
    fn has_builtin_timeout(&self) -> bool {
        false
    }

    /// Best-effort cleanup invoked by the gateway when its defensive outer
    /// timeout fires while [`ask`](Self::ask) is still pending. The
    /// `ask` future has already been dropped by the time this is called,
    /// so any external pending state it registered (e.g. an SSE
    /// interaction store entry) must be removed here to prevent a stale
    /// entry from silently swallowing a late user reply. Default no-op;
    /// handles that maintain such external state override this.
    async fn abort_pending(&self, _request: &InteractionRequest) {}
}
