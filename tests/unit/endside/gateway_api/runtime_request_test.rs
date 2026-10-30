use super::{
    evaluate_max_tokens_against_window, resolve_visible_tool_names, resolve_window_with_probe,
    tui_entry_context, ResolvedWindow,
};
use crate::app_state::AppState;
use crate::config::{AgentRoleConfig, Config};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
fn trusted_window_rejects_max_tokens_at_or_above_window() {
    let error = evaluate_max_tokens_against_window(
        ResolvedWindow::Trusted { window: 128_000 },
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
fn trusted_window_accepts_max_tokens_below_window() {
    let outcome = evaluate_max_tokens_against_window(
        ResolvedWindow::Trusted { window: 128_000 },
        8_192,
        "glm-5.2",
        std::path::Path::new("/tmp/config.toml"),
    );
    assert_eq!(outcome, Ok(()));
}

#[test]
fn estimated_window_never_rejects_even_when_max_tokens_exceeds_it() {
    // The static estimate can undershoot the real window; rejecting on it
    // would block a working config whenever probing is offline or slow.
    let outcome = evaluate_max_tokens_against_window(
        ResolvedWindow::Estimated { window: 128_000 },
        200_000,
        "glm-5.2",
        std::path::Path::new("/tmp/config.toml"),
    );
    assert_eq!(outcome, Ok(()));
}

#[tokio::test]
async fn probe_success_within_budget_resolves_trusted() {
    let resolved = resolve_window_with_probe(
        unique_cache_key("probe-success"),
        Some(128_000),
        async { Some(200_000) },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resolved, Some(ResolvedWindow::Trusted { window: 200_000 }));
}

#[tokio::test]
async fn failed_probe_falls_back_to_static_estimate() {
    let resolved = resolve_window_with_probe(
        unique_cache_key("probe-none"),
        Some(128_000),
        async { None },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(
        resolved,
        Some(ResolvedWindow::Estimated { window: 128_000 })
    );
}

#[tokio::test]
async fn negative_result_hit_reuses_estimate_without_reprobing() {
    let key = unique_cache_key("cache-hit-negative");
    // Prime the entry with a negative probe result first.
    let _ = resolve_window_with_probe(
        key.clone(),
        Some(128_000),
        async { None },
        Duration::from_secs(1),
    )
    .await;

    let resolved = resolve_window_with_probe(
        key,
        Some(128_000),
        async { panic!("a valid negative entry must not probe again within the TTL") },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(
        resolved,
        Some(ResolvedWindow::Estimated { window: 128_000 })
    );
}

#[tokio::test]
async fn unresolved_entry_fails_fast_without_reprobing() {
    let key = unique_cache_key("unresolved");
    let probes = Arc::new(AtomicUsize::new(0));
    let probe_counter = probes.clone();

    let first = resolve_window_with_probe(
        key.clone(),
        None,
        async move {
            probe_counter.fetch_add(1, Ordering::SeqCst);
            None
        },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(first, None);

    let second = resolve_window_with_probe(
        key,
        None,
        async { panic!("unresolved entries must fail fast within the TTL") },
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(second, None);
    assert_eq!(probes.load(Ordering::SeqCst), 1);
}
