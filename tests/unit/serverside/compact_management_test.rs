use super::compact_report;
use crate::daemon_config::DaemonConfig;

#[test]
fn reports_effective_defaults_without_a_compact_section() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n")
        .expect("write config");
    let report = compact_report(&DaemonConfig::load_from(&path).expect("config"));
    assert!(!report.configured);
    assert!(report.valid);
    assert_eq!(report.effective.warning_ratio, 0.6);
    assert_eq!(report.effective.blocking_ratio, 0.9);
}

#[test]
fn rejects_invalid_threshold_order_and_zero_budgets() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[compact]\nwarning_ratio = 0.8\nauto_compact_ratio = 0.5\nblocking_ratio = 0.9\nsummary_max_tokens = 0\n",
    )
    .expect("write config");
    let report = compact_report(&DaemonConfig::load_from(&path).expect("config"));
    assert!(!report.valid);
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.code == "invalid_threshold_order"));
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.path == "compact.summary_max_tokens"));
}
