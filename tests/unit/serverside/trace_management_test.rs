use super::trace_report;
use crate::daemon_config::{AppConfig, DaemonConfig};
use std::path::PathBuf;
use tempfile::TempDir;

fn config(content: &str) -> DaemonConfig {
    DaemonConfig {
        app: toml::from_str::<AppConfig>(content).expect("config"),
        config_path: PathBuf::from("/tmp/config.toml"),
    }
}

#[test]
fn reports_non_database_backends_without_paths() {
    let report = trace_report(&config(
        "[llm]\nprovider='local'\nmodel='test'\n[trace]\nstorage_backend='stdout'\n",
    ));
    assert_eq!(report.storage_backend, "stdout");
    assert!(report.readable);
    assert!(report.db_path.is_none());
    assert!(report.recent.is_empty());
}

#[test]
fn missing_database_is_ready_without_creating_it() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("trace.db");
    let report = trace_report(&config(&format!(
        "[llm]\nprovider='local'\nmodel='test'\n[trace]\nstorage_backend='moirai-sqlite'\ndb_path='{}'\n",
        path.display()
    )));
    assert!(report.readable);
    assert!(!report.db_exists);
    assert!(!path.exists());
}
