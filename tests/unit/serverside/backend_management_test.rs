use super::backend_report;
use crate::daemon_config::DaemonConfig;

#[test]
fn default_backend_is_local_and_does_not_expose_options() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n").expect("config");
    let report = backend_report(&DaemonConfig::load_from(&path).expect("load"));
    assert_eq!(report.kind, "local");
    assert!(report.valid);
    assert!(report.option_keys.is_empty());
}

#[test]
fn e2b_report_exposes_key_availability_but_not_value() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n[server.operation_backend]\nkind = \"e2b\"\n[server.operation_backend.options]\napi_key = \"must-not-leak\"\ntemplate_id = \"base\"\n").expect("config");
    let report = backend_report(&DaemonConfig::load_from(&path).expect("load"));
    let json = serde_json::to_string(&report).expect("json");
    assert_eq!(report.api_key_available, Some(true));
    assert_eq!(report.api_key_source, Some("inline"));
    assert!(!json.contains("must-not-leak"));
}
