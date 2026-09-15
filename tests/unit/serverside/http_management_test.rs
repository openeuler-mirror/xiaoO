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
