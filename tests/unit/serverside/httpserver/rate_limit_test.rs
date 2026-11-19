use super::*;

#[test]
fn default_and_empty_toml_agree_on_sane_defaults() {
    let cfg = RateLimitConfig::default();
    assert!(cfg.enabled);
    assert_eq!(cfg.requests_per_second, 2);
    assert_eq!(cfg.burst, 10);
    assert!(cfg.routes.is_empty());

    let cfg: RateLimitConfig = toml::from_str("").unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.requests_per_second, 2);
    assert_eq!(cfg.burst, 10);
}

/// Disabled and zero-quota settings must not install a governor layer.
#[test]
fn degenerate_configs_yield_no_layer() {
    for cfg in [
        RateLimitConfig {
            enabled: false,
            ..Default::default()
        },
        RateLimitConfig {
            requests_per_second: 0,
            ..Default::default()
        },
        RateLimitConfig {
            burst: 0,
            ..Default::default()
        },
    ] {
        assert!(cfg.governor_layer().is_none());
    }
}

#[test]
fn enabled_config_yields_a_layer() {
    assert!(RateLimitConfig::default().governor_layer().is_some());
}

#[test]
fn full_toml_with_overrides() {
    let raw = r#"
            enabled = true
            requests_per_second = 5
            burst = 20

            [routes.health]
            requests_per_second = 10
            burst = 30

            [routes.chat]
            requests_per_second = 1
            burst = 5
        "#;
    let cfg: RateLimitConfig = toml::from_str(raw).unwrap();
    assert!(cfg.enabled);
    assert_eq!((cfg.requests_per_second, cfg.burst), (5, 20));
    assert_eq!(cfg.routes.len(), 2);
    assert_eq!(
        (
            cfg.routes["health"].requests_per_second,
            cfg.routes["health"].burst
        ),
        (10, 30)
    );
    assert_eq!(
        (
            cfg.routes["chat"].requests_per_second,
            cfg.routes["chat"].burst
        ),
        (1, 5)
    );
}

#[test]
fn effective_limit_falls_back_when_route_absent() {
    let cfg = RateLimitConfig::default();
    assert_eq!(cfg.effective_limit("chat"), (2, 10));
    assert_eq!(cfg.effective_limit("unknown"), (2, 10));
}

#[test]
fn effective_limit_uses_override_when_present() {
    let mut routes = BTreeMap::new();
    routes.insert(
        "health".into(),
        RouteRateLimitOverride {
            requests_per_second: 100,
            burst: 200,
        },
    );
    let cfg = RateLimitConfig {
        routes,
        ..Default::default()
    };
    assert_eq!(cfg.effective_limit("health"), (100, 200));
    assert_eq!(cfg.effective_limit("chat"), (2, 10));
}
