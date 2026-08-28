//! Per-session attach lease table.
//!
//! Enforces "one session = one writer at a time" across multiple TUI processes
//! sharing the same daemon. Lives in-memory; on daemon restart the table is
//! cleared (TUIs re-acquire via the next `open_session` / heartbeat).
//!
//! The lease does NOT lock the session handle: in-flight turns started by a
//! previous holder run to completion. Only *new* turn submissions are gated —
//! by the router's `assert_lease_holder` check and by the `SessionActor`'s
//! `pending_turns` pop-time check.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Number of shards in a [`SessionLeaseTable`]. Each shard owns its own
/// `Mutex<HashMap>` so lease operations on different `session_id`s never
/// contend on the same lock.
const LEASE_TABLE_SHARD_COUNT: usize = 16;

/// Heartbeat staleness threshold (45 s = 3x the 15 s heartbeat interval, the
/// safety margin against transient network blips). A lease older than this is
/// orphaned and may be taken over.
pub(crate) const STALE_LEASE_THRESHOLD_MS: u64 = 45_000;

/// Reaper threshold (2 h): a session with no live lease for longer than this
/// (and no in-flight turn) is force-closed to reclaim leaked backends.
/// Conservative so a user who detaches and comes back an hour later still
/// finds the session warm.
pub(crate) const ORPHAN_SESSION_THRESHOLD_MS: u64 = 7_200_000;

/// Reaper sweep interval. `spawn_orphan_reaper` and any future observability
/// surface share this single source of truth.
pub(crate) const REAPER_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(600);

// TUI-side HTTP timeouts live in `apps/endside/src/gateway_api/http_timeouts.rs`
// so this crate does not carry TUI-only configuration.

/// Prefix for daemon-internal principal ids (cron / hooks / channel ingress).
/// Daemon principals don't acquire leases — they're cooperative background
/// callers trusted not to race user turns. The prefix lets
/// `assert_lease_holder` and the actor's pop-time guard recognise them and
/// bypass the holder check explicitly (rather than the old implicit
/// `client_id = None` bypass that was unauditable).
pub(crate) const DAEMON_PRINCIPAL_PREFIX: &str = "daemon:";

/// Whether `client_id` is a daemon-internal principal. Daemon principals
/// bypass the lease-holder check at the router, the close path, and the
/// actor's pop-time guard; the bypass is logged for audit.
pub fn is_daemon_principal(client_id: &str) -> bool {
    client_id.starts_with(DAEMON_PRINCIPAL_PREFIX)
}

/// Build a daemon-internal principal id from its `suffix`.
fn daemon_principal(suffix: &str) -> String {
    format!("{DAEMON_PRINCIPAL_PREFIX}{suffix}")
}

/// Principal id for the cron scheduler's daemon-internal turns.
pub fn daemon_cron_principal() -> String {
    daemon_principal("cron")
}

/// Principal id for a hook action's daemon-internal turn.
pub fn daemon_hook_principal(hook_id: &str) -> String {
    daemon_principal(&format!("hook:{hook_id}"))
}

/// Principal id for a channel-ingress daemon-internal turn.
pub fn daemon_channel_principal(channel_id: &str) -> String {
    daemon_principal(&format!("channel:{channel_id}"))
}

#[derive(Debug, Clone)]
pub(crate) struct SessionLease {
    pub client_id: String,
    pub client_pid: Option<u32>,
    pub client_hostname: Option<String>,
    pub last_heartbeat_ms: u64,
}

impl SessionLease {
    /// Whether this lease is older than `threshold_ms` relative to `now`.
    pub(crate) fn is_stale(&self, now: u64, threshold_ms: u64) -> bool {
        if self.last_heartbeat_ms == 0 {
            return true;
        }
        now.saturating_sub(self.last_heartbeat_ms) > threshold_ms
    }
}

/// Outcome of a lease-acquire attempt.
#[derive(Debug, Clone)]
pub(crate) enum LeaseAcquireOutcome {
    /// This client had no prior lease, held it already (refreshed), or took
    /// over a stale lease. The operation may proceed.
    Acquired,
    /// Another live client holds the lease.
    Busy {
        holder_client_id: String,
        holder_hostname: Option<String>,
        holder_pid: Option<u32>,
        last_heartbeat_ms: u64,
        /// `true` iff the holder's heartbeat is older than
        /// `STALE_LEASE_THRESHOLD_MS` at the moment of the check. Lets a TUI
        /// auto-retry (the daemon would itself take over a stale lease on
        /// the next `acquire`, but surfacing this hint avoids a wasted
        /// round-trip).
        stale: bool,
    },
    /// The daemon's wall clock is broken (`SystemTime::now() < UNIX_EPOCH`).
    /// Lease enforcement refuses to proceed rather than silently relaxing
    /// single-writer.
    ClockSkew,
}

#[derive(Clone)]
pub(crate) struct SessionLeaseTable {
    /// Per-session lease table, sharded by `session_id` hash.
    shards: Arc<[Mutex<HashMap<String, SessionLease>>]>,
}

impl SessionLeaseTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Pick the shard owning `session_id` (stable within a process via
    /// `DefaultHasher`).
    fn shard_for(&self, session_id: &str) -> &Mutex<HashMap<String, SessionLease>> {
        let mut hasher = DefaultHasher::new();
        session_id.hash(&mut hasher);
        let idx = (hasher.finish() as usize) % self.shards.len();
        &self.shards[idx]
    }

    /// Acquire / refresh the lease on `session_id` for `client_id`. Atomic
    /// under the shard's mutex; concurrent calls for the same session
    /// serialise, different sessions run in parallel. A live lease held by a
    /// different `client_id` is never overridden (returns `Busy`); stale
    /// leases are taken over unconditionally.
    pub async fn acquire(
        &self,
        session_id: &str,
        client_id: &str,
        client_pid: Option<u32>,
        client_hostname: Option<String>,
    ) -> LeaseAcquireOutcome {
        let now = match current_time_ms() {
            Ok(now) => now,
            Err(_) => return LeaseAcquireOutcome::ClockSkew,
        };
        let mut table = self.shard_for(session_id).lock().await;
        match table.get_mut(session_id) {
            Some(lease) if lease.client_id == client_id => {
                lease.last_heartbeat_ms = now;
                // Refresh display identity only when the caller provides a
                // value — a `None` (e.g. hostname resolution failed) must
                // not erase the previously-recorded identity.
                if client_pid.is_some() {
                    lease.client_pid = client_pid;
                }
                if client_hostname.is_some() {
                    lease.client_hostname = client_hostname;
                }
                LeaseAcquireOutcome::Acquired
            }
            Some(lease) if lease.is_stale(now, STALE_LEASE_THRESHOLD_MS) => {
                *lease = fresh_lease(client_id.to_string(), client_pid, client_hostname, now);
                LeaseAcquireOutcome::Acquired
            }
            Some(lease) => LeaseAcquireOutcome::Busy {
                holder_client_id: lease.client_id.clone(),
                holder_hostname: lease.client_hostname.clone(),
                holder_pid: lease.client_pid,
                last_heartbeat_ms: lease.last_heartbeat_ms,
                stale: false,
            },
            None => {
                table.insert(
                    session_id.to_string(),
                    fresh_lease(client_id.to_string(), client_pid, client_hostname, now),
                );
                LeaseAcquireOutcome::Acquired
            }
        }
    }

    /// Renew an existing lease. `Ok(())` when the caller is the current
    /// holder or when no lease is in place (e.g. after daemon restart wiped
    /// the table — in that case the heartbeat auto-re-acquires the lease so
    /// the TUI keeps running without re-calling `open_session`). `Err(Busy)`
    /// when the lease was taken over by another `client_id`.
    ///
    /// The `stale` field of the failure is computed while the mutex is held,
    /// so a TUI deciding whether to auto-retry acts on the daemon's actual
    /// rejection state, not a stale snapshot. `client_pid` / `client_hostname`
    /// are only used on the auto-re-acquire path (so holder identity survives
    /// a daemon restart); ignored on the refresh path.
    pub async fn heartbeat(
        &self,
        session_id: &str,
        client_id: &str,
        client_pid: Option<u32>,
        client_hostname: Option<String>,
    ) -> Result<(), LeaseCheckFailure> {
        let now = match current_time_ms() {
            Ok(now) => now,
            Err(_) => return Err(LeaseCheckFailure::ClockSkew),
        };
        let mut table = self.shard_for(session_id).lock().await;
        match table.get_mut(session_id) {
            Some(lease) if lease.client_id == client_id => {
                lease.last_heartbeat_ms = now;
                Ok(())
            }
            Some(lease) => {
                // Compute staleness under the mutex so the caller sees the
                // exact state the daemon used to reject.
                let stale = lease.is_stale(now, STALE_LEASE_THRESHOLD_MS);
                Err(LeaseCheckFailure::Busy {
                    holder_client_id: lease.client_id.clone(),
                    holder_hostname: lease.client_hostname.clone(),
                    holder_pid: lease.client_pid,
                    last_heartbeat_ms: lease.last_heartbeat_ms,
                    stale,
                })
            }
            None => {
                // No lease in the table (daemon restarted, or lease was
                // detached): auto-re-acquire so the caller skips a
                // round-trip through `open_session`. Stamp pid / hostname
                // so holder identity survives a daemon restart.
                table.insert(
                    session_id.to_string(),
                    fresh_lease(client_id.to_string(), client_pid, client_hostname, now),
                );
                Ok(())
            }
        }
    }

    /// Release the lease if held by `client_id`. Idempotent: returns `Ok(())`
    /// when the caller doesn't hold the lease. Does NOT touch the session or
    /// its backend.
    pub async fn detach(&self, session_id: &str, client_id: &str) {
        let mut table = self.shard_for(session_id).lock().await;
        if matches!(table.get(session_id), Some(lease) if lease.client_id == client_id) {
            table.remove(session_id);
        }
    }

    /// Whether `session_id` currently has a live lease (heartbeat fresher
    /// than `stale_threshold_ms`). Used by the orphan reaper's TOCTOU
    /// re-check — reads the **current** table state under the mutex so a
    /// heartbeat between sweep-start and `mark_closing` doesn't get missed.
    /// Fails open (`false`) on clock skew.
    pub(crate) async fn has_live_lease(&self, session_id: &str, stale_threshold_ms: u64) -> bool {
        let Ok(now) = current_time_ms() else {
            return false;
        };
        let table = self.shard_for(session_id).lock().await;
        table
            .get(session_id)
            .map(|lease| !lease.is_stale(now, stale_threshold_ms))
            .unwrap_or(false)
    }

    /// Read-only lease check. `Ok(())` iff `client_id` is the current holder,
    /// no lease is in place, or the recorded holder's heartbeat is stale.
    ///
    /// **Does NOT mutate the table** — use from call sites that only need an
    /// authorisation decision (e.g. the SessionActor's pop-time check, which
    /// must not steal the lease from a freshly-staled holder). For the takeover
    /// side effect, use [`check_or_takeover_holder`](Self::check_or_takeover_holder).
    ///
    /// `None` callers never match the recorded holder (a holder always has a
    /// non-empty `client_id`); they pass through on the stale / no-lease arms.
    pub(crate) async fn check_holder(
        &self,
        session_id: &str,
        client_id: Option<&str>,
    ) -> Result<(), LeaseCheckFailure> {
        let now = match current_time_ms() {
            Ok(now) => now,
            Err(_) => return Err(LeaseCheckFailure::ClockSkew),
        };
        let table = self.shard_for(session_id).lock().await;
        let Some(lease) = table.get(session_id) else {
            return Ok(());
        };
        // Anonymous (`None`) callers never match the recorded holder; they
        // fall through to the stale / Busy arms below.
        let is_holder = client_id.is_some_and(|cid| lease.client_id == cid);
        if is_holder || lease.is_stale(now, STALE_LEASE_THRESHOLD_MS) {
            Ok(())
        } else {
            Err(LeaseCheckFailure::Busy {
                holder_client_id: lease.client_id.clone(),
                holder_hostname: lease.client_hostname.clone(),
                holder_pid: lease.client_pid,
                last_heartbeat_ms: lease.last_heartbeat_ms,
                stale: false,
            })
        }
    }

    /// Lease check with stale-takeover side effect. `Ok(())` iff the caller
    /// is the holder, no lease is in place, or the recorded lease is stale.
    ///
    /// **Mutates the table on the stale path**: for an identified caller the
    /// stale entry is rewritten to point at the caller (matching `acquire`'s
    /// stale-takeover arm) so subsequent heartbeats succeed without
    /// re-acquire. For an anonymous caller the stale entry is left in place
    /// and the RPC is allowed through.
    ///
    /// `None` callers never match the recorded holder (a holder always has a
    /// non-empty `client_id`); they pass through on the stale / no-lease arms.
    pub(crate) async fn check_or_takeover_holder(
        &self,
        session_id: &str,
        client_id: Option<&str>,
    ) -> Result<(), LeaseCheckFailure> {
        let now = match current_time_ms() {
            Ok(now) => now,
            Err(_) => return Err(LeaseCheckFailure::ClockSkew),
        };
        let mut table = self.shard_for(session_id).lock().await;
        let Some(lease) = table.get(session_id) else {
            return Ok(());
        };
        // Anonymous (`None`) callers never match the recorded holder; they
        // fall through to the stale / Busy arms below.
        let is_holder = client_id.is_some_and(|cid| lease.client_id == cid);
        if is_holder {
            return Ok(());
        }
        if lease.is_stale(now, STALE_LEASE_THRESHOLD_MS) {
            // Stale lease: take it over for an identified caller (so
            // the next heartbeat succeeds), then allow the RPC through.
            if let Some(cid) = client_id.filter(|s| !s.is_empty()) {
                table.insert(
                    session_id.to_string(),
                    fresh_lease(cid.to_string(), None, None, now),
                );
            }
            return Ok(());
        }
        Err(LeaseCheckFailure::Busy {
            holder_client_id: lease.client_id.clone(),
            holder_hostname: lease.client_hostname.clone(),
            holder_pid: lease.client_pid,
            last_heartbeat_ms: lease.last_heartbeat_ms,
            stale: false,
        })
    }

    /// Remove the lease for `session_id` unconditionally (after
    /// `force_close_session` so a future `open_session` for the same id
    /// starts clean).
    pub async fn remove(&self, session_id: &str) {
        let mut table = self.shard_for(session_id).lock().await;
        table.remove(session_id);
    }

    /// Snapshot for the GC reaper: `(session_id, holder_client_id,
    /// last_heartbeat_ms)` across all shards. Locks each shard in turn; a
    /// sweep longer than one heartbeat tick may see a slightly stale view of
    /// later shards, but the per-record TOCTOU re-check in
    /// [`reap_one_record`] guards against acting on it.
    pub async fn snapshot(&self) -> Vec<(String, String, u64)> {
        let mut out = Vec::new();
        for shard in self.shards.iter() {
            let table = shard.lock().await;
            for (sid, lease) in table.iter() {
                out.push((
                    sid.clone(),
                    lease.client_id.clone(),
                    lease.last_heartbeat_ms,
                ));
            }
        }
        out
    }
}

impl Default for SessionLeaseTable {
    fn default() -> Self {
        let shards: Vec<Mutex<HashMap<String, SessionLease>>> = (0..LEASE_TABLE_SHARD_COUNT)
            .map(|_| Mutex::new(HashMap::new()))
            .collect();
        let shards: Arc<[Mutex<HashMap<String, SessionLease>>]> = Arc::from(shards);
        Self { shards }
    }
}

#[cfg(test)]
impl SessionLeaseTable {
    /// Test-only: backdate the recorded heartbeat for `session_id`. Production
    /// code MUST NOT use this — it bypasses the heartbeat staleness invariant.
    pub(crate) async fn set_last_heartbeat_ms_for_test(&self, session_id: &str, ms: u64) {
        let mut table = self.shard_for(session_id).lock().await;
        if let Some(lease) = table.get_mut(session_id) {
            lease.last_heartbeat_ms = ms;
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum LeaseCheckFailure {
    /// The session is held by another live `client_id`.
    Busy {
        holder_client_id: String,
        holder_hostname: Option<String>,
        holder_pid: Option<u32>,
        last_heartbeat_ms: u64,
        stale: bool,
    },
    /// The daemon's wall clock is broken (`SystemTime::now() < UNIX_EPOCH`).
    /// Lease enforcement refuses to proceed rather than silently relaxing
    /// single-writer.
    ClockSkew,
}

/// Wall-clock error returned when `SystemTime::now()` reports a time before
/// `UNIX_EPOCH` (misconfigured RTC, VM clock skew, first boot before NTP).
/// Lease enforcement fails-closed on this — see [`current_time_ms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockSkew;

impl std::fmt::Display for ClockSkew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("daemon wall clock is before UNIX_EPOCH; lease enforcement is fail-closed")
    }
}

impl std::error::Error for ClockSkew {}

/// Current wall time in milliseconds since `UNIX_EPOCH`. `Err(ClockSkew)` when
/// `SystemTime::now()` reports a time before `UNIX_EPOCH`. Callers MUST
/// propagate the error — lease enforcement fails-closed until NTP corrects
/// the clock.
pub(crate) fn current_time_ms() -> Result<u64, ClockSkew> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|_| {
            tracing::error!(
                "SystemTime::now() returned a time before UNIX_EPOCH; \
                 lease enforcement is fail-closed (acquire / heartbeat / \
                 holder checks are rejected) until the clock is corrected"
            );
            ClockSkew
        })
}

/// Construct a fresh lease for `client_id` stamped at `now`. Centralises the
/// `last_heartbeat_ms == now` invariant shared by the `acquire` / `heartbeat`
/// / `check_or_takeover_holder` takeover paths.
fn fresh_lease(
    client_id: String,
    client_pid: Option<u32>,
    client_hostname: Option<String>,
    now: u64,
) -> SessionLease {
    SessionLease {
        client_id,
        client_pid,
        client_hostname,
        last_heartbeat_ms: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acquire_then_busy_for_other_client() {
        let table = SessionLeaseTable::new();
        let outcome_a = table
            .acquire("s1", "client-a", Some(1), Some("host-a".to_string()))
            .await;
        assert!(matches!(outcome_a, LeaseAcquireOutcome::Acquired));

        let outcome_b = table
            .acquire("s1", "client-b", Some(2), Some("host-b".to_string()))
            .await;
        match outcome_b {
            LeaseAcquireOutcome::Busy {
                holder_client_id, ..
            } => assert_eq!(holder_client_id, "client-a"),
            other => panic!("expected Busy, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stale_lease_taken_over_without_force() {
        let table = SessionLeaseTable::new();
        let _ = table.acquire("s1", "client-a", Some(1), None).await;
        // Manually backdate the heartbeat past the staleness threshold.
        table
            .set_last_heartbeat_ms_for_test(
                "s1",
                current_time_ms()
                    .expect("wall clock")
                    .saturating_sub(STALE_LEASE_THRESHOLD_MS + 1_000),
            )
            .await;
        let outcome = table.acquire("s1", "client-b", Some(2), None).await;
        assert!(matches!(outcome, LeaseAcquireOutcome::Acquired));
    }

    #[tokio::test]
    async fn heartbeat_returns_error_when_taken_over() {
        // After a stale-takeover the previous holder's heartbeat must fail
        // with `Busy{holder=client-b, stale=false}`.
        let table = SessionLeaseTable::new();
        let _ = table.acquire("s1", "client-a", Some(1), None).await;
        assert!(table.heartbeat("s1", "client-a", None, None).await.is_ok());
        // Backdate client-a's lease so client-b can take it over via the
        // stale path (live leases are never overridden).
        table
            .set_last_heartbeat_ms_for_test(
                "s1",
                current_time_ms()
                    .expect("wall clock")
                    .saturating_sub(STALE_LEASE_THRESHOLD_MS + 1_000),
            )
            .await;
        let _ = table.acquire("s1", "client-b", Some(2), None).await;
        // client-a heartbeat now fails
        let result = table.heartbeat("s1", "client-a", None, None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LeaseCheckFailure::Busy {
                holder_client_id,
                stale: false,
                ..
            } => assert_eq!(holder_client_id, "client-b"),
            other => panic!("expected Busy(holder=client-b, stale=false), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn detach_only_releases_self_lease() {
        let table = SessionLeaseTable::new();
        let _ = table.acquire("s1", "client-a", Some(1), None).await;
        // client-b trying to detach client-a's lease is a no-op.
        table.detach("s1", "client-b").await;
        assert!(table.check_holder("s1", Some("client-a")).await.is_ok());
        // client-a detaching its own lease works.
        table.detach("s1", "client-a").await;
        // After detach the table is empty: check_holder returns Ok for any
        // caller (no lease in place).
        assert!(table.check_holder("s1", Some("client-a")).await.is_ok());
        assert!(table.check_holder("s1", Some("client-b")).await.is_ok());
        // A fresh acquire by client-b succeeds (table was empty).
        let outcome = table.acquire("s1", "client-b", Some(2), None).await;
        assert!(matches!(outcome, LeaseAcquireOutcome::Acquired));
    }

    #[tokio::test]
    async fn check_holder_returns_ok_when_no_lease() {
        let table = SessionLeaseTable::new();
        assert!(table.check_holder("s1", Some("client-a")).await.is_ok());
        assert!(table.check_holder("s1", None).await.is_ok());
    }

    #[tokio::test]
    async fn check_holder_does_not_take_over_stale_lease() {
        // Read-only check: a stale lease returns Ok but the table is NOT
        // mutated — the contract used by the SessionActor's pop-time check.
        let table = SessionLeaseTable::new();
        let _ = table.acquire("s1", "client-a", Some(1), None).await;
        // Backdate the lease past the staleness threshold.
        table
            .set_last_heartbeat_ms_for_test(
                "s1",
                current_time_ms()
                    .expect("wall clock")
                    .saturating_sub(STALE_LEASE_THRESHOLD_MS + 1_000),
            )
            .await;
        // Identified non-holder caller: Ok (stale → allow), but no takeover.
        assert!(table.check_holder("s1", Some("client-b")).await.is_ok());
        // client-a is STILL the recorded holder (stale): client-b's
        // heartbeat hits the "client_id differs" arm and returns Err.
        let result = table.heartbeat("s1", "client-b", None, None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LeaseCheckFailure::Busy {
                holder_client_id,
                stale: true,
                ..
            } => assert_eq!(holder_client_id, "client-a"),
            other => panic!("expected Busy(holder=client-a, stale=true), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn check_or_takeover_holder_takes_over_stale_lease_for_identified_caller() {
        let table = SessionLeaseTable::new();
        let _ = table.acquire("s1", "client-a", Some(1), None).await;
        // Backdate the lease past the staleness threshold.
        table
            .set_last_heartbeat_ms_for_test(
                "s1",
                current_time_ms()
                    .expect("wall clock")
                    .saturating_sub(STALE_LEASE_THRESHOLD_MS + 1_000),
            )
            .await;
        // An identified caller's `check_or_takeover_holder` succeeds AND
        // takes over the lease: the new caller is now the holder.
        assert!(table
            .check_or_takeover_holder("s1", Some("client-b"))
            .await
            .is_ok());
        // client-b now holds the lease — client-a's heartbeat returns Err.
        let result = table.heartbeat("s1", "client-a", None, None).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LeaseCheckFailure::Busy {
                holder_client_id, ..
            } => assert_eq!(holder_client_id, "client-b"),
            other => panic!("expected Busy(holder=client-b), got {other:?}"),
        }
        // client-b's heartbeat succeeds.
        assert!(table.heartbeat("s1", "client-b", None, None).await.is_ok());
    }

    #[tokio::test]
    async fn check_or_takeover_holder_allows_anonymous_caller_through_stale_lease_without_takeover()
    {
        // Anonymous caller is allowed through a stale lease but does NOT
        // take it over, so an identified caller can still claim it later.
        let table = SessionLeaseTable::new();
        let _ = table.acquire("s1", "client-a", Some(1), None).await;
        table
            .set_last_heartbeat_ms_for_test(
                "s1",
                current_time_ms()
                    .expect("wall clock")
                    .saturating_sub(STALE_LEASE_THRESHOLD_MS + 1_000),
            )
            .await;
        assert!(table.check_or_takeover_holder("s1", None).await.is_ok());
        let snapshot = table.snapshot().await;
        let (_, holder, _) = snapshot
            .iter()
            .find(|(sid, _, _)| sid == "s1")
            .expect("lease still recorded");
        assert_eq!(holder, "client-a", "anonymous caller must not take over");
    }
}
