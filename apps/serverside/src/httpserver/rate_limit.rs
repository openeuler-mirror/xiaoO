use axum::response::IntoResponse;
use governor::middleware::NoOpMiddleware;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::PeerIpKeyExtractor, GovernorError,
    GovernorLayer,
};

/// Convenience type alias hiding tower-governor generics.
pub type RateLimitLayer = GovernorLayer<PeerIpKeyExtractor, NoOpMiddleware>;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RateLimitConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rps")]
    pub requests_per_second: u32,
    #[serde(default = "default_burst")]
    pub burst: u32,
    #[serde(default)]
    pub routes: BTreeMap<String, RouteRateLimitOverride>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RouteRateLimitOverride {
    #[serde(default = "default_rps")]
    pub requests_per_second: u32,
    #[serde(default = "default_burst")]
    pub burst: u32,
}

fn default_enabled() -> bool {
    true
}
fn default_rps() -> u32 {
    2
}
fn default_burst() -> u32 {
    10
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: 2,
            burst: 10,
            routes: BTreeMap::new(),
        }
    }
}

impl RateLimitConfig {
    /// Build a [`GovernorLayer`] from this configuration.
    ///
    /// Returns `None` when disabled or when rps/burst are zero.
    /// Uses peer-IP identification via [`PeerIpKeyExtractor`]; behind a reverse
    /// proxy all clients share the proxy's quota unless you configure
    /// `into_make_service_with_connect_info::<SocketAddr>()` in `main.rs`.
    pub fn governor_layer(&self) -> Option<RateLimitLayer> {
        if !self.enabled {
            return None;
        }
        if self.requests_per_second == 0 || self.burst == 0 {
            return None;
        }
        let config = Arc::new(
            GovernorConfigBuilder::default()
                .per_second(self.requests_per_second as u64)
                .burst_size(self.burst)
                .error_handler(|e| match e {
                    GovernorError::TooManyRequests { headers, .. } => {
                        let wait = headers
                            .as_ref()
                            .and_then(|h| h.get("x-ratelimit-after"))
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(1);
                        (
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            [
                                ("Retry-After", wait.to_string()),
                                ("X-RateLimit-Remaining", "0".to_string()),
                            ],
                            axum::Json(serde_json::json!({
                                "error": format!("rate limit exceeded; retry after {}s", wait),
                            })),
                        )
                            .into_response()
                    }
                    GovernorError::UnableToExtractKey | GovernorError::Other { .. } => (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        axum::Json(serde_json::json!({
                            "error": "unable to extract client identity for rate limiting",
                        })),
                    )
                        .into_response(),
                })
                .finish()
                .expect("non-zero rps and burst guarantee valid config"),
        );
        Some(GovernorLayer { config })
    }

    /// Per-route limit lookup; falls back to global defaults when route key is absent.
    pub fn effective_limit(&self, route_key: &str) -> (u32, u32) {
        self.routes
            .get(route_key)
            .map(|o| (o.requests_per_second, o.burst))
            .unwrap_or((self.requests_per_second, self.burst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_enabled_with_sane_defaults() {
        let cfg = RateLimitConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.requests_per_second, 2);
        assert_eq!(cfg.burst, 10);
        assert!(cfg.routes.is_empty());
    }

    #[test]
    fn disabled_config_yields_no_layer() {
        let cfg = RateLimitConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(cfg.governor_layer().is_none());
    }

    #[test]
    fn enabled_config_yields_a_layer() {
        assert!(RateLimitConfig::default().governor_layer().is_some());
    }

    #[test]
    fn zero_rps_yields_no_layer() {
        let cfg = RateLimitConfig {
            requests_per_second: 0,
            ..Default::default()
        };
        assert!(cfg.governor_layer().is_none());
    }

    #[test]
    fn zero_burst_yields_no_layer() {
        let cfg = RateLimitConfig {
            burst: 0,
            ..Default::default()
        };
        assert!(cfg.governor_layer().is_none());
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let cfg: RateLimitConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.enabled, true);
        assert_eq!(cfg.requests_per_second, 2);
        assert_eq!(cfg.burst, 10);
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
}
