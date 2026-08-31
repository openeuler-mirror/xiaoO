use serde::Serialize;

use crate::config_inspect::ConfigInspectOverrides;
use crate::daemon_config::DaemonConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HttpReport {
    pub schema_version: u32,
    pub runtime_host: String,
    pub runtime_port: u16,
    pub bearer: BearerStatus,
    pub rate_limit: RateLimitStatus,
    pub dashboard: DashboardStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BearerStatus {
    pub source: String,
    pub environment: Option<String>,
    pub available: bool,
    pub inline_secret: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RateLimitStatus {
    pub configured: bool,
    pub enabled: bool,
    pub requests_per_second: u32,
    pub burst: u32,
    pub routes: Vec<RouteLimitStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RouteLimitStatus {
    pub name: String,
    pub requests_per_second: u32,
    pub burst: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DashboardStatus {
    pub configured: bool,
    pub enabled: bool,
    pub host: String,
    pub port: Option<u16>,
    pub overridden_by_cli: bool,
}

pub fn http_report(config: &DaemonConfig, overrides: &ConfigInspectOverrides) -> HttpReport {
    let bearer = bearer_status(config, overrides);
    let rate_limit = config.app.http.rate_limit.as_ref();
    let mut routes = rate_limit
        .map(|limit| {
            limit
                .routes
                .iter()
                .map(|(name, route)| RouteLimitStatus {
                    name: name.clone(),
                    requests_per_second: route.requests_per_second,
                    burst: route.burst,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    routes.sort_by(|left, right| left.name.cmp(&right.name));
    let dashboard_configured = config.app.http.dashboard.is_some();
    let dashboard_overridden = overrides.no_dashboard
        || overrides.dashboard_host.is_some()
        || overrides.dashboard_port.is_some();
    let dashboard_enabled = !overrides.no_dashboard
        && config
            .app
            .http
            .dashboard
            .as_ref()
            .is_none_or(|dashboard| dashboard.is_enabled());
    HttpReport {
        schema_version: 1,
        runtime_host: overrides.daemon_host.clone(),
        runtime_port: overrides.daemon_port,
        bearer,
        rate_limit: RateLimitStatus {
            configured: rate_limit.is_some(),
            enabled: rate_limit.is_some_and(|limit| {
                limit.enabled && limit.requests_per_second > 0 && limit.burst > 0
            }),
            requests_per_second: rate_limit.map_or(2, |limit| limit.requests_per_second),
            burst: rate_limit.map_or(10, |limit| limit.burst),
            routes,
        },
        dashboard: DashboardStatus {
            configured: dashboard_configured,
            enabled: dashboard_enabled,
            host: config.dashboard_host(overrides.dashboard_host.clone()),
            port: if overrides.no_dashboard {
                None
            } else {
                config.dashboard_port(overrides.dashboard_port)
            },
            overridden_by_cli: dashboard_overridden,
        },
    }
}

fn bearer_status(config: &DaemonConfig, overrides: &ConfigInspectOverrides) -> BearerStatus {
    let (source, environment, inline_secret) = if let Some(environment) = overrides
        .bearer_token_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        ("cli_environment", Some(environment.to_string()), false)
    } else if let Some(environment) = config
        .app
        .http
        .bearer_token_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        (
            "configuration_environment",
            Some(environment.to_string()),
            false,
        )
    } else if config
        .app
        .http
        .bearer_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        ("configuration_inline", None, true)
    } else {
        ("none", None, false)
    };
    let available = environment
        .as_deref()
        .is_some_and(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
        || inline_secret;
    BearerStatus {
        source: source.to_string(),
        environment,
        available,
        inline_secret,
    }
}

#[cfg(test)]
mod tests {
    use super::http_report;
    use crate::config_inspect::ConfigInspectOverrides;
    use crate::daemon_config::{AppConfig, DaemonConfig};
    use std::path::PathBuf;

    fn config(content: &str) -> DaemonConfig {
        DaemonConfig {
            app: toml::from_str::<AppConfig>(content).expect("config"),
            config_path: PathBuf::from("/tmp/config.toml"),
        }
    }

    #[test]
    fn reports_safe_defaults_without_secrets() {
        let report = http_report(
            &config("[llm]\nprovider='local'\nmodel='test'\n"),
            &ConfigInspectOverrides {
                daemon_host: "127.0.0.1".to_string(),
                daemon_port: 18080,
                ..ConfigInspectOverrides::default()
            },
        );
        assert_eq!(report.runtime_port, 18080);
        assert_eq!(report.bearer.source, "none");
        assert!(!report.rate_limit.configured);
        assert!(!report.rate_limit.enabled);
        assert!(report.dashboard.enabled);
        assert_eq!(report.dashboard.port, Some(28081));
    }

    #[test]
    fn applies_cli_overrides_and_redacts_inline_token() {
        let config = config(
            "[llm]\nprovider='local'\nmodel='test'\n\
             [http]\nbearer_token='never-return-this'\n\
             [http.rate_limit]\nenabled=true\nrequests_per_second=5\nburst=20\n\
             [http.rate_limit.routes.health]\nrequests_per_second=10\nburst=30\n\
             [http.dashboard]\nenabled=true\nhost='0.0.0.0'\nport=29000\n",
        );
        let report = http_report(
            &config,
            &ConfigInspectOverrides {
                daemon_host: "127.0.0.2".to_string(),
                daemon_port: 19000,
                no_dashboard: true,
                ..ConfigInspectOverrides::default()
            },
        );
        assert_eq!(report.runtime_host, "127.0.0.2");
        assert_eq!(report.runtime_port, 19000);
        assert!(report.bearer.inline_secret);
        assert!(report.bearer.available);
        assert_eq!(report.rate_limit.routes[0].name, "health");
        assert!(!report.dashboard.enabled);
        assert_eq!(report.dashboard.port, None);
        assert!(report.dashboard.overridden_by_cli);
        let json = serde_json::to_string(&report).expect("report JSON");
        assert!(!json.contains("never-return-this"));
    }
}
