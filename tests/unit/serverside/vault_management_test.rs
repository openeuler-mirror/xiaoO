use super::vault_report;
use crate::daemon_config::{AppConfig, DaemonConfig};
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn reports_sources_without_exposing_secret_values() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    let secret = "vault-report-secret";
    std::fs::write(
        &path,
        format!(
            "[llm]\nprovider='openrouter'\nmodel='test'\napi_key_env='VAULT_REPORT_KEY'\n[http]\nbearer_token='{secret}'\n"
        ),
    )
    .expect("write config");
    xiaoo_shared::llm_secrets::save_llm_secret(&path, "VAULT_REPORT_KEY", secret)
        .expect("save secret");
    let report = vault_report(&DaemonConfig {
        app: toml::from_str::<AppConfig>(&std::fs::read_to_string(&path).unwrap()).unwrap(),
        config_path: path,
    });
    assert!(report.store_readable);
    assert_eq!(report.references.len(), 2);
    assert!(report.references.iter().any(|reference| {
        reference.path == "llm.api_key_env" && reference.source == "encrypted_store"
    }));
    let json = serde_json::to_string(&report).expect("serialize report");
    assert!(!json.contains(secret));
}

#[test]
fn reports_missing_sdf_library_as_unavailable() {
    let config = DaemonConfig {
        app: toml::from_str::<AppConfig>(
            "[llm]\nprovider='ollama'\nmodel='test'\n[vault]\nenabled=true\nuse_sdf=true\n",
        )
        .unwrap(),
        config_path: PathBuf::from("/tmp/config.toml"),
    };
    let report = vault_report(&config);
    assert_eq!(report.provider, "sdf");
    assert_eq!(report.sdf_available, report.sdf_library_path.is_file());
}

#[test]
fn reports_effective_default_service_secret_references() {
    let config = DaemonConfig {
        app: toml::from_str::<AppConfig>(
            "[llm]\nprovider='ollama'\nmodel='test'\n[mcp_server]\nenabled=true\n[server.operation_backend]\nkind='e2b'\n",
        )
        .unwrap(),
        config_path: PathBuf::from("/tmp/config.toml"),
    };
    let report = vault_report(&config);
    for environment in [
        "XIAOO_MCP_CHATBOT_TOKEN",
        "XIAOO_MCP_AGENT_TOKEN",
        "E2B_API_KEY",
    ] {
        assert!(report
            .references
            .iter()
            .any(|reference| { reference.environment.as_deref() == Some(environment) }));
    }
}
