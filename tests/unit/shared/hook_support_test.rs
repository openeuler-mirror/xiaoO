use super::{hook_catalog, test_hook};
use agent_types::common::HookerId;
use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};

#[test]
fn reports_registered_hooks_and_effective_state() {
    let mut config = HookerRegistryConfig {
        default: HookerDefaultMode::None,
        ..HookerRegistryConfig::default()
    };
    config
        .enabled
        .push(HookerId("builtin_session_created_hooker".to_string()));
    config.policies.insert(
        HookerId("builtin_session_created_hooker".to_string()),
        serde_json::json!({ "message": "ready" }),
    );

    let report = hook_catalog(&config).expect("hook catalog");
    assert_eq!(report.schema_version, 1);
    assert_eq!(report.default_mode, "none");
    let hook = report
        .hooks
        .iter()
        .find(|hook| hook.id == "builtin_session_created_hooker")
        .expect("session hook");
    assert!(hook.enabled);
    assert!(hook.has_policy);
    assert_eq!(hook.category, "session_created");
}

#[tokio::test]
async fn tests_registered_hook_with_synthetic_runtime_input() {
    let config = HookerRegistryConfig::default();
    let report = test_hook(&config, "builtin_session_created_hooker")
        .await
        .expect("hook test");

    assert!(report.success);
    assert_eq!(report.category, "session_created");
    assert_eq!(report.output_kind, Some("session_created"));
    assert_eq!(report.action_count, 0);
    assert!(report.error_kind.is_none());
}
