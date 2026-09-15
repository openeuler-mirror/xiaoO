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
async fn check_or_takeover_holder_allows_anonymous_caller_through_stale_lease_without_takeover() {
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

impl super::SessionLeaseTable {
    /// Test-only: backdate the recorded heartbeat for `session_id`. Production
    /// code MUST NOT use this — it bypasses the heartbeat staleness invariant.
    pub(crate) async fn set_last_heartbeat_ms_for_test(&self, session_id: &str, ms: u64) {
        let mut table = self.shard_for(session_id).lock().await;
        if let Some(lease) = table.get_mut(session_id) {
            lease.last_heartbeat_ms = ms;
        }
    }
}
