use super::{
    evaluate_max_tokens_against_window, resolve_visible_tool_names, resolve_window_with_probe,
    tui_entry_context, ProbeConclusion,
};
use crate::app_state::AppState;
use crate::config::{AgentRoleConfig, Config};
use crate::gateway_api::window_disk_cache::tests::test_support::{
    corrupt_disk_at, disk_at, disk_with_entry,
};
use crate::gateway_api::window_disk_cache::{WindowDiskCache, NEGATIVE_TTL};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use xiaoo_api::chat::DEFAULT_SYSTEM_TOKEN_RESERVE;

/// The window cache is process-global; test-specific prefixes plus a
/// monotonic counter keep concurrent test cases from sharing entries.
fn unique_cache_key(label: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!(
        "unit-test|{}|{}|{}",
        label,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

/// Memory-tier-only resolve: the disk layer stays disabled so tests never
/// touch the real cache directory. Disk-tier cases use [`resolve_with_disk`].
async fn resolve_memory_only(
    cache_key: String,
    static_window: Option<usize>,
    probe: impl Future<Output = ProbeConclusion> + Send + 'static,
    budget: Duration,
) -> Option<usize> {
    resolve_window_with_probe(
        cache_key,
        static_window,
        probe,
        budget,
        WindowDiskCache::disabled(),
    )
    .await
}

/// Disk-tier fixture: an isolated tempdir, a handle into it, and a unique
/// key. The TempDir rides in the returned tuple so it outlives the case.
fn disk_fixture(label: &str) -> (tempfile::TempDir, WindowDiskCache, String) {
    let home = tempfile::tempdir().unwrap();
    let disk = disk_at(&home);
    (home, disk, unique_cache_key(label))
}

/// Disk-tier resolve: the static estimate is fixed at 128_000 (the cases
/// assert against it), so only the probe conclusion and budget vary.
async fn resolve_with_disk(
    key: &str,
    disk: WindowDiskCache,
    probe: impl Future<Output = ProbeConclusion> + Send + 'static,
    budget: Duration,
) -> Option<usize> {
    resolve_window_with_probe(key.to_string(), Some(128_000), probe, budget, disk).await
}

#[tokio::test]
async fn resolve_visible_tool_names_requires_exact_tool_names() {
    let mut config = Config::default();
    config.agent.insert(
        "code-reviewer".to_string(),
        AgentRoleConfig {
            description: String::new(),
            prompt: None,
            max_turns: None,
            tools: BTreeMap::from([
                ("write".to_string(), false),
                ("file_write".to_string(), false),
            ]),
        },
    );

    let mut state = AppState::new_with_config(
        &config,
        PathBuf::from("config.toml"),
        PathBuf::from("."),
        None,
    )
    .expect("app state should initialize");
    state.active_agent_role = Some("code-reviewer".to_string());

    let visible = resolve_visible_tool_names(
        &state.agent_config,
        state.active_agent_role.as_deref(),
        &state.workspace,
    )
    .await
    .expect("tool visibility should resolve");
    let visible: BTreeSet<_> = visible.into_iter().collect();

    assert!(visible.contains("file_edit"));
    assert!(!visible.contains("file_write"));
}

#[test]
fn tui_entry_context_carries_active_agent_role() {
    let mut config = Config::default();
    config.agent.insert(
        "plan".to_string(),
        AgentRoleConfig {
            description: String::new(),
            prompt: None,
            max_turns: None,
            tools: BTreeMap::new(),
        },
    );
    let mut state = AppState::new_with_config(
        &config,
        PathBuf::from("config.toml"),
        PathBuf::from("."),
        None,
    )
    .expect("app state should initialize");
    state.active_agent_role = Some("plan".to_string());

    let entry = tui_entry_context(&state, None);

    assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
}

#[test]
fn tui_entry_context_carries_instance_id_for_remote() {
    let config = Config::default();
    let mut state = AppState::new_with_config(
        &config,
        PathBuf::from("config.toml"),
        PathBuf::from("."),
        None,
    )
    .expect("app state should initialize");
    state.active_agent_role = Some("plan".to_string());

    let entry = tui_entry_context(&state, Some("http://127.0.0.1:18080".to_string()));

    // Both the role and the remote instance_id must be set so the
    // daemon can resolve [agent.<name>] (max_turns) in remote mode.
    assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
    assert_eq!(entry.instance_id.as_deref(), Some("http://127.0.0.1:18080"));
}

#[test]
fn window_rejects_max_tokens_at_or_above_window() {
    let error = evaluate_max_tokens_against_window(
        128_000,
        128_000,
        "glm-5.2",
        std::path::Path::new("/tmp/config.toml"),
    )
    .expect_err("max_tokens == window leaves no input space");
    assert!(error.contains("NO space for input"));
    assert!(error.contains("glm-5.2"));
    assert!(error.contains("/tmp/config.toml"));
}

#[test]
fn window_with_input_space_accepts_max_tokens() {
    let outcome = evaluate_max_tokens_against_window(
        128_000,
        8_192,
        "glm-5.2",
        std::path::Path::new("/tmp/config.toml"),
    );
    assert_eq!(outcome, Ok(()));
}

#[test]
fn window_rejects_max_tokens_far_beyond_the_window() {
    // Regression: used to panic deep in the turn instead of failing here.
    let error = evaluate_max_tokens_against_window(
        128_000,
        100_000_000,
        "glm-5.2",
        std::path::Path::new("/tmp/config.toml"),
    )
    .expect_err("no window bounds a budget this large");
    assert!(error.contains("NO space for input"));
    assert!(error.contains("100000000"));
}

#[test]
fn window_reserve_band_rejects_max_tokens_just_below_the_window() {
    let error = evaluate_max_tokens_against_window(
        128_000,
        127_000,
        "glm-5.2",
        std::path::Path::new("/tmp/config.toml"),
    )
    .expect_err("the system reserve leaves no input space");
    assert!(error.contains("NO space for input"));
    // The reserve value must surface in the text, or a rejection below the
    // window reads as a contradiction (127000 < 128000 yet rejected).
    assert!(error.contains(&DEFAULT_SYSTEM_TOKEN_RESERVE.to_string()));
}

#[tokio::test]
async fn probe_success_within_budget_resolves_probed_window() {
    let resolved = resolve_memory_only(
        unique_cache_key("probe-success"),
        Some(128_000),
        async { ProbeConclusion::Window(200_000) },
        Duration::from_secs(1),
    )
    .await;
    // 200_000 is the probe's answer; only the probe path can produce it.
    assert_eq!(resolved, Some(200_000));
}

#[tokio::test]
async fn failed_probe_falls_back_to_static_estimate() {
    let resolved = resolve_memory_only(
        unique_cache_key("probe-none"),
        Some(128_000),
        async { ProbeConclusion::Failed },
        Duration::from_secs(1),
    )
    .await;
    // 128_000 is the static window; the probe concluded nothing.
    assert_eq!(resolved, Some(128_000));
}

#[tokio::test]
async fn negative_result_hit_reuses_estimate_without_reprobing() {
    let key = unique_cache_key("cache-hit-negative");
    // Prime the entry with a definitive miss first.
    let _ = resolve_memory_only(
        key.clone(),
        Some(128_000),
        async { ProbeConclusion::NotFound },
        Duration::from_secs(1),
    )
    .await;

    let resolved = resolve_memory_only(
        key,
        Some(128_000),
        async { panic!("a valid negative entry must not probe again within the TTL") },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(128_000));
}

#[tokio::test]
async fn unresolved_entry_fails_fast_without_reprobing() {
    let key = unique_cache_key("unresolved");
    let probes = Arc::new(AtomicUsize::new(0));
    let probe_counter = probes.clone();

    let first = resolve_memory_only(
        key.clone(),
        None,
        async move {
            probe_counter.fetch_add(1, Ordering::SeqCst);
            ProbeConclusion::NotFound
        },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(first, None);

    let second = resolve_memory_only(
        key,
        None,
        async { panic!("unresolved entries must fail fast within the TTL") },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(second, None);
    assert_eq!(probes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disk_positive_entry_adopts_without_probing() {
    let (_home, disk, key) = disk_fixture("disk-positive");
    disk.store(&key, Some(200_000));

    // A short budget proves the adoption is immediate.
    let resolved = resolve_with_disk(
        &key,
        disk,
        async { panic!("a fresh disk entry must resolve without probing") },
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(resolved, Some(200_000));
}

#[tokio::test]
async fn disk_negative_entry_adopts_without_probing() {
    let (_home, disk, key) = disk_fixture("disk-negative");
    disk.store(&key, None);

    let resolved = resolve_with_disk(
        &key,
        disk,
        async { panic!("a fresh disk negative must resolve without probing") },
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(resolved, Some(128_000));
}

#[tokio::test]
async fn probe_conclusion_persists_to_disk() {
    let (_home, disk, key) = disk_fixture("disk-write");

    let resolved = resolve_with_disk(
        &key,
        disk.clone(),
        async { ProbeConclusion::Window(200_000) },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(200_000));

    // run_probe writes the disk tier before waking waiters.
    assert_eq!(disk.load(&key), Some(Some(200_000)));
}

#[tokio::test]
async fn negative_conclusion_persists_to_disk() {
    let (_home, disk, key) = disk_fixture("disk-write-negative");

    let resolved = resolve_with_disk(
        &key,
        disk.clone(),
        async { ProbeConclusion::NotFound },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(128_000));

    assert_eq!(disk.load(&key), Some(None));
}

#[tokio::test]
async fn transient_failure_stays_memory_only() {
    let (_home, disk, key) = disk_fixture("disk-write-failed");

    let resolved = resolve_with_disk(
        &key,
        disk.clone(),
        async { ProbeConclusion::Failed },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(128_000));

    // A transient failure must not reach the disk tier: behind the long
    // negative TTL it would keep suppressing re-probes long after the
    // network recovers. The memory tier's short TTL self-heals instead.
    assert_eq!(disk.load(&key), None);
}

#[tokio::test]
async fn expired_disk_negative_reprobes() {
    let home = tempfile::tempdir().unwrap();
    let key = unique_cache_key("disk-expired-negative");
    let age = NEGATIVE_TTL.as_secs() as i64 + 3600;
    // An expired disk entry is no conclusion at all: the chain re-probes.
    let disk = disk_with_entry(&home, &key, None, age);

    let resolved = resolve_with_disk(
        &key,
        disk.clone(),
        async { ProbeConclusion::Window(200_000) },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(200_000));
    assert_eq!(disk.load(&key), Some(Some(200_000)));
}

#[tokio::test]
async fn corrupt_disk_file_falls_through_to_probe_and_repairs() {
    let home = tempfile::tempdir().unwrap();
    let disk = corrupt_disk_at(&home);
    let key = unique_cache_key("disk-corrupt");

    let resolved = resolve_with_disk(
        &key,
        disk.clone(),
        async { ProbeConclusion::Window(200_000) },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(200_000));
    // The conclusion write replaces the corrupt file wholesale.
    assert_eq!(disk.load(&key), Some(Some(200_000)));
}

#[tokio::test]
async fn budget_cutoff_writes_no_disk_entry() {
    let (_home, disk, key) = disk_fixture("disk-cutoff");

    let resolved = resolve_with_disk(
        &key,
        disk.clone(),
        async {
            tokio::time::sleep(Duration::from_millis(600)).await;
            ProbeConclusion::Window(200_000)
        },
        Duration::from_millis(30),
    )
    .await;
    assert_eq!(resolved, Some(128_000));

    // Pin the cut-off turn's own side of the invariant before the detached
    // probe lands: a wrong entry written here (static estimate or negative
    // conclusion) would be overwritten — and masked — by the probe's later
    // store, then go stale behind the disk TTLs (7d positive / 24d negative).
    assert_eq!(
        disk.load(&key),
        None,
        "the cut-off turn must not persist any conclusion"
    );

    // The detached probe persists the real conclusion when it finishes.
    for _ in 0..100 {
        if disk.load(&key) == Some(Some(200_000)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("background probe did not persist its conclusion");
}
