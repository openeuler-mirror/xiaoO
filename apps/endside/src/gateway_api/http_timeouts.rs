//! TUI-side HTTP timeouts for outbound calls to the daemon.
//!
//! Bounding all outbound HTTP protects the event loop from the OS's 75 s+
//! TCP timeout. The values live in the TUI crate (not `xiaoo_shared`) to keep
//! TUI-only config out of the shared crate; the daemon-side
//! [`STALE_LEASE_THRESHOLD_MS`](xiaoo_shared::gateway::STALE_LEASE_THRESHOLD_MS)
//! remains the single source of truth for the staleness threshold.

/// Heartbeat interval; 3x safety margin under the daemon's 45 s staleness
/// threshold ([`STALE_LEASE_THRESHOLD_MS`](xiaoo_shared::gateway::STALE_LEASE_THRESHOLD_MS)).
pub(crate) const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Per-call timeout for the TUI's `/runtimes/heartbeat` RPC. A timeout is
/// reported as `HeartbeatError::Network` and retried next tick.
pub(crate) const HEARTBEAT_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Timeout for the TUI's `/runtimes/open` RPC, which may involve backend
/// provisioning (e.g. leasing an e2b sandbox).
pub(crate) const OPEN_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Timeout wrapping `detach` / `close` on exit / `/new` / `/remote off` so
/// shutdown / session-switch never blocks more than 5 s on an unreachable
/// daemon.
pub(crate) const EXIT_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Safety-net timeout for `post_json` so fire-and-forget callers (cancel,
/// interaction) cannot hang. Tighter caller-side timeouts (e.g.
/// [`EXIT_RPC_TIMEOUT`]) fire first.
pub(crate) const POST_JSON_SAFETY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connect-phase timeout for the TUI's shared `reqwest::Client`. Only the
/// connect phase is bounded — the SSE turn stream is a long-lived response
/// body and must not be cut by a per-request timeout. Operators can override
/// via `XIAOO_HTTP_CONNECT_TIMEOUT_SECS` (positive whole seconds; invalid /
/// unset falls back to this default).
pub(crate) const HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolve the connect-phase timeout from `XIAOO_HTTP_CONNECT_TIMEOUT_SECS`
/// (positive whole seconds), falling back to [`HTTP_CONNECT_TIMEOUT`] on
/// unset / unparsable / non-positive values.
///
/// `0` is explicitly rejected: `reqwest::connect_timeout(Duration::ZERO)`
/// makes every connect attempt fail instantly, silently breaking all
/// outbound HTTP.
pub(crate) fn resolve_http_connect_timeout() -> std::time::Duration {
    match std::env::var("XIAOO_HTTP_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(secs) if secs > 0 => std::time::Duration::from_secs(secs),
        Some(rejected) => {
            tracing::warn!(
                value = %rejected,
                fallback = ?HTTP_CONNECT_TIMEOUT,
                "XIAOO_HTTP_CONNECT_TIMEOUT_SECS must be a positive whole number of seconds; \
                 a zero value would make every connect fail instantly. Falling back to default"
            );
            HTTP_CONNECT_TIMEOUT
        }
        None => HTTP_CONNECT_TIMEOUT,
    }
}
