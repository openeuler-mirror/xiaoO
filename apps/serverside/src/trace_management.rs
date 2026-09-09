use serde::Serialize;
use std::path::PathBuf;

use crate::daemon_config::DaemonConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TraceReport {
    pub schema_version: u32,
    pub configured: bool,
    pub storage_backend: String,
    pub db_path: Option<PathBuf>,
    pub db_exists: bool,
    pub db_size_bytes: Option<u64>,
    pub readable: bool,
    pub error_kind: Option<String>,
    pub recent: Vec<xiaoo_shared::trace_support::TraceDiagnosticSummary>,
}

pub fn trace_report(config: &DaemonConfig) -> TraceReport {
    let configured = config.app.trace.is_some();
    let storage_backend = config
        .app
        .trace
        .as_ref()
        .and_then(|trace| trace.storage_backend.as_deref())
        .unwrap_or("moirai-sqlite")
        .trim()
        .to_string();
    if storage_backend != "moirai-sqlite" {
        return TraceReport {
            schema_version: 1,
            configured,
            readable: matches!(storage_backend.as_str(), "stdout" | "noop"),
            error_kind: (!matches!(storage_backend.as_str(), "stdout" | "noop"))
                .then(|| "unsupported_backend".to_string()),
            storage_backend,
            db_path: None,
            db_exists: false,
            db_size_bytes: None,
            recent: Vec::new(),
        };
    }

    let db_path = config
        .app
        .trace
        .as_ref()
        .and_then(|trace| trace.db_path.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(xiaoo_shared::trace_support::default_trace_db_path);
    let db_path = if db_path.is_absolute() {
        db_path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(db_path)
    };
    let metadata = std::fs::metadata(&db_path).ok();
    if metadata.is_none() {
        return TraceReport {
            schema_version: 1,
            configured,
            storage_backend,
            db_path: Some(db_path),
            db_exists: false,
            db_size_bytes: None,
            readable: true,
            error_kind: None,
            recent: Vec::new(),
        };
    }
    if !metadata.as_ref().is_some_and(|metadata| metadata.is_file()) {
        return TraceReport {
            schema_version: 1,
            configured,
            storage_backend,
            db_path: Some(db_path),
            db_exists: true,
            db_size_bytes: metadata.map(|metadata| metadata.len()),
            readable: false,
            error_kind: Some("not_a_file".to_string()),
            recent: Vec::new(),
        };
    }
    match xiaoo_shared::trace_support::inspect_trace_database(&db_path, 20) {
        Ok(recent) => TraceReport {
            schema_version: 1,
            configured,
            storage_backend,
            db_path: Some(db_path),
            db_exists: true,
            db_size_bytes: metadata.map(|metadata| metadata.len()),
            readable: true,
            error_kind: None,
            recent,
        },
        Err(error) => TraceReport {
            schema_version: 1,
            configured,
            storage_backend,
            db_path: Some(db_path),
            db_exists: true,
            db_size_bytes: metadata.map(|metadata| metadata.len()),
            readable: false,
            error_kind: Some(error_kind(&error).to_string()),
            recent: Vec::new(),
        },
    }
}

fn error_kind(error: &str) -> &'static str {
    if error.contains("schema") {
        "schema_unavailable"
    } else if error.contains("decode") {
        "invalid_metadata"
    } else if error.contains("query") {
        "query_failed"
    } else {
        "open_failed"
    }
}

#[cfg(test)]
mod tests {
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
}
