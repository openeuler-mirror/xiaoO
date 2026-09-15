use super::*;
use tempfile::TempDir;

/// Build a counter backed by a fresh temporary directory so the test
/// never touches the shared `~/.xiaoo/sandbox_counts.json` used by a
/// running daemon. The returned `TempDir` must outlive the counter.
fn temp_counter() -> (TempDir, SandboxCounter) {
    let dir = TempDir::new().expect("tempdir");
    let counter = SandboxCounter::new_with_storage_dir(
        MAX_ACTIVE_SANDBOXES_PER_KEY,
        dir.path().to_path_buf(),
    );
    (dir, counter)
}

/// Force the most recent pending reservation for `key` to be stale so
/// the next `load_data` GCs it, simulating a build that exceeded
/// `PENDING_RESERVATION_TTL_MS`.
async fn force_pending_stale(counter: &SandboxCounter, key: &SandboxCounterKey) {
    let _in_process = IN_PROCESS_LOCK.lock().await;
    let mut data = counter.load_data().expect("load");
    let stale_ms = current_time_ms().saturating_sub(PENDING_RESERVATION_TTL_MS + 1_000);
    if let Some(reservations) = data.pending_reservations.get_mut(&key.to_string_key()) {
        if let Some(last) = reservations.last_mut() {
            last.reserved_at_ms = stale_ms;
        }
    }
    counter.save_data(&data).expect("save stale");
}

/// Read the per-key ghost count directly from the data file (test-only
/// inspector; production code never reads `ghosts` directly).
async fn ghost_count(counter: &SandboxCounter, key: &SandboxCounterKey) -> usize {
    let _in_process = IN_PROCESS_LOCK.lock().await;
    let data = counter.load_data().expect("load");
    data.ghosts.get(&key.to_string_key()).copied().unwrap_or(0)
}

#[test]
fn key_identifier_is_hashed_and_deterministic() {
    // The raw identifier must NOT appear in the stored form.
    let key = SandboxCounterKey::new("e2b", "secret-api-key-12345");
    assert_ne!(
        key.key_identifier, "secret-api-key-12345",
        "key_identifier must be hashed, not stored as plaintext"
    );
    // Same plaintext → same derived form (deterministic lookup).
    let again = SandboxCounterKey::new("e2b", "secret-api-key-12345");
    assert_eq!(key.key_identifier, again.key_identifier);
    // Different plaintext → different derived form.
    let other = SandboxCounterKey::new("e2b", "different-api-key-67890");
    assert_ne!(key.key_identifier, other.key_identifier);
    // The derived form is 43 chars (32 bytes base64url-no-pad).
    assert_eq!(key.key_identifier.len(), 43);
    // The plaintext must not leak through `to_string_key` either.
    assert!(!key.to_string_key().contains("secret-api-key-12345"));
}

#[tokio::test]
async fn test_check_and_reserve_at_limit() {
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-limit");

    for _ in 0..MAX_ACTIVE_SANDBOXES_PER_KEY {
        counter
            .check_and_reserve(&key)
            .await
            .expect("should reserve");
        counter
            .confirm_creation(&key)
            .await
            .expect("should confirm");
    }

    let result = counter.check_and_reserve(&key).await;
    assert!(matches!(
        result,
        Err(SandboxCounterError::LimitExceeded { .. })
    ));

    for _ in 0..MAX_ACTIVE_SANDBOXES_PER_KEY {
        counter.release(&key).await.expect("should release");
    }
}

#[tokio::test]
async fn test_confirm_and_release() {
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-confirm");

    counter
        .check_and_reserve(&key)
        .await
        .expect("should reserve");
    counter
        .confirm_creation(&key)
        .await
        .expect("should confirm");

    let count = counter.get_count(&key).await.expect("should get count");
    assert_eq!(count, 1);

    counter.release(&key).await.expect("should release");
    let count = counter.get_count(&key).await.expect("should get count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_pending_counts() {
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-pending");

    counter
        .check_and_reserve(&key)
        .await
        .expect("should reserve");

    let pending = counter
        .get_pending_count(&key)
        .await
        .expect("should get pending");
    assert_eq!(pending, 1);

    let total = counter
        .get_total_count(&key)
        .await
        .expect("should get total");
    assert_eq!(total, 1);

    counter
        .cancel_reservation(&key)
        .await
        .expect("should cancel");

    let pending = counter
        .get_pending_count(&key)
        .await
        .expect("should get pending");
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn test_in_process_lock_protection() {
    let (_dir, counter) = temp_counter();
    let counter = Arc::new(counter);
    let key = SandboxCounterKey::new("e2b", "test-concurrent");

    let mut tasks = Vec::new();
    for _ in 0..5 {
        let counter_clone = counter.clone();
        let key_clone = key.clone();
        let task = tokio::spawn(async move { counter_clone.check_and_reserve(&key_clone).await });
        tasks.push(task);
    }

    let results: Vec<_> = futures::future::join_all(tasks).await;

    let success_count = results
        .iter()
        .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
        .count();
    assert!(success_count <= MAX_ACTIVE_SANDBOXES_PER_KEY);

    for _ in 0..success_count {
        counter
            .cancel_reservation(&key)
            .await
            .expect("should cancel");
    }
}

#[tokio::test]
async fn test_stale_pending_reservations_are_gc_on_load() {
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-stale-gc");

    // Seed a stale pending reservation directly into the data file.
    {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let stale_ms = current_time_ms().saturating_sub(PENDING_RESERVATION_TTL_MS + 1_000);
        let data = SandboxCounterData {
            counts: HashMap::new(),
            pending_reservations: HashMap::from([(
                key.to_string_key(),
                vec![
                    PendingReservation {
                        reserved_at_ms: stale_ms,
                    },
                    PendingReservation {
                        reserved_at_ms: stale_ms,
                    },
                ],
            )]),
            ghosts: HashMap::new(),
        };
        counter.save_data(&data).expect("save stale data");
    }

    // The stale reservations should be GC'd on load, so the pending
    // count reports 0 and a fresh reservation succeeds.
    let pending = counter
        .get_pending_count(&key)
        .await
        .expect("should get pending after gc");
    assert_eq!(pending, 0, "stale reservations should have been gc'd");

    counter
        .check_and_reserve(&key)
        .await
        .expect("should reserve after gc freed the slot");
    counter
        .cancel_reservation(&key)
        .await
        .expect("should cancel");
}

#[tokio::test]
async fn test_fresh_pending_reservations_survive_gc() {
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-fresh-pending");

    counter
        .check_and_reserve(&key)
        .await
        .expect("should reserve");

    let pending = counter
        .get_pending_count(&key)
        .await
        .expect("should get pending");
    assert_eq!(pending, 1, "fresh reservation should survive gc");

    counter
        .cancel_reservation(&key)
        .await
        .expect("should cancel");
}

#[tokio::test]
async fn test_confirm_after_pending_gc_does_not_breach_limit() {
    // A slow build (> PENDING_RESERVATION_TTL_MS) has its reservation
    // GC'd, the freed slot is reused until the pool is full, then the
    // slow build completes and calls confirm_creation. `counts` must
    // NOT exceed max_per_key; the over-limit sandbox is accounted in
    // `ghosts`, and `release` consumes the ghost credit so `counts`
    // never dips below the real live count (no cascade breach).
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-gc-confirm-full");

    // Reserve a slot for the "slow build".
    counter
        .check_and_reserve(&key)
        .await
        .expect("should reserve slow build");

    // Force the reservation stale so the next load_data GCs it.
    force_pending_stale(&counter, &key).await;

    // Trigger GC; the stale reservation is reclaimed.
    let pending = counter
        .get_pending_count(&key)
        .await
        .expect("should get pending");
    assert_eq!(pending, 0, "stale reservation should be gc'd");

    // Another process fills the pool to the limit, reusing the freed slot.
    for _ in 0..MAX_ACTIVE_SANDBOXES_PER_KEY {
        counter
            .check_and_reserve(&key)
            .await
            .expect("should reserve");
        counter
            .confirm_creation(&key)
            .await
            .expect("should confirm");
    }

    // The slow build finally completes. Its reservation is gone and
    // the pool is full, so confirm_creation cannot increment `counts`
    // (that would breach the limit). Instead it records a ghost
    // credit: the real sandbox exists and must be tracked, but in
    // `ghosts` so `counts <= max_per_key` holds.
    counter
        .confirm_creation(&key)
        .await
        .expect("should confirm");

    let count = counter.get_count(&key).await.expect("should get count");
    assert_eq!(
        count, MAX_ACTIVE_SANDBOXES_PER_KEY,
        "counts must not breach max_per_key; the ghost is in `ghosts`"
    );
    assert_eq!(
        ghost_count(&counter, &key).await,
        1,
        "the ghost sandbox must be accounted in ghosts"
    );

    // While the ghost is live, new reservations must be rejected so
    // the over-limit does not cascade into a further sandbox.
    let result = counter.check_and_reserve(&key).await;
    assert!(
        matches!(result, Err(SandboxCounterError::LimitExceeded { .. })),
        "check_and_reserve must reject while a ghost is live"
    );

    // Releasing the ghost consumes the ghost credit (not `counts`), so
    // `counts` stays at the limit and the bookkeeping self-corrects.
    counter.release(&key).await.expect("should release ghost");
    assert_eq!(
        counter.get_count(&key).await.expect("count"),
        MAX_ACTIVE_SANDBOXES_PER_KEY,
        "ghost release must not decrement counts"
    );
    assert_eq!(
        ghost_count(&counter, &key).await,
        0,
        "ghost credit consumed"
    );

    // A subsequent normal release now drops `counts` below the limit.
    counter.release(&key).await.expect("should release normal");
    assert_eq!(
        counter.get_count(&key).await.expect("count"),
        MAX_ACTIVE_SANDBOXES_PER_KEY - 1
    );
}

#[tokio::test]
async fn test_confirm_after_pending_gc_increments_when_under_limit() {
    // When the reservation was GC'd but the pool is NOT full,
    // confirm_creation should still increment the confirmed count so
    // the real sandbox is tracked.
    let (_dir, counter) = temp_counter();
    let key = SandboxCounterKey::new("e2b", "test-gc-confirm-under");

    counter
        .check_and_reserve(&key)
        .await
        .expect("should reserve slow build");

    // Force the reservation stale and trigger GC.
    force_pending_stale(&counter, &key).await;
    let pending = counter
        .get_pending_count(&key)
        .await
        .expect("should get pending");
    assert_eq!(pending, 0, "stale reservation should be gc'd");

    // Pool is empty, so confirm_creation should still increment.
    counter
        .confirm_creation(&key)
        .await
        .expect("should confirm");

    let count = counter.get_count(&key).await.expect("should get count");
    assert_eq!(
        count, 1,
        "confirm after GC should still count the sandbox when under the limit"
    );
}

#[test]
fn load_max_sandbox_cnt_from_path_parses_value() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("sandbox.toml");
    std::fs::write(&path, "max_sandbox_cnt = 7\n").expect("write config");

    assert_eq!(load_max_sandbox_cnt_from_path(&path), Some(7));
}

#[test]
fn load_max_sandbox_cnt_from_path_missing_file_returns_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("does-not-exist.toml");

    assert_eq!(load_max_sandbox_cnt_from_path(&path), None);
}

#[test]
fn load_max_sandbox_cnt_from_path_absent_field_returns_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("sandbox.toml");
    std::fs::write(&path, "[other]\nkey = 1\n").expect("write config");

    assert_eq!(load_max_sandbox_cnt_from_path(&path), None);
}

#[test]
fn load_max_sandbox_cnt_from_path_invalid_toml_returns_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("sandbox.toml");
    std::fs::write(&path, "not = valid = toml\n").expect("write config");

    assert_eq!(load_max_sandbox_cnt_from_path(&path), None);
}

impl super::SandboxCounter {
    pub(crate) async fn get_count(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<usize, SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;
        Ok(data.counts.get(&key.to_string_key()).copied().unwrap_or(0))
    }
}

impl super::SandboxCounter {
    pub(crate) async fn get_pending_count(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<usize, SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;
        Ok(data
            .pending_reservations
            .get(&key.to_string_key())
            .map(Vec::len)
            .unwrap_or(0))
    }
}
