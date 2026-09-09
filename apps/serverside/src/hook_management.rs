use crate::daemon_config::DaemonConfig;
use anyhow::Result;
use xiaoo_shared::hook_support::HookCatalogReport;

pub fn hook_catalog(config: &DaemonConfig) -> Result<HookCatalogReport> {
    let mut report = xiaoo_shared::hook_support::hook_catalog(&config.app.hooker)?;
    let backend = config
        .app
        .trace
        .as_ref()
        .and_then(|trace| trace.storage_backend.as_deref())
        .unwrap_or("moirai-sqlite")
        .trim();
    if backend != "moirai-sqlite" {
        report.recent_error_kind = Some("trace_backend_unavailable".to_string());
        return Ok(report);
    }
    let path = config
        .app
        .trace
        .as_ref()
        .and_then(|trace| trace.db_path.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(xiaoo_shared::trace_support::default_trace_db_path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(path)
    };
    if !path.is_file() {
        return Ok(report);
    }
    match xiaoo_shared::trace_support::inspect_recent_hook_executions(&path, 20) {
        Ok(recent) => report.recent_executions = recent,
        Err(_) => report.recent_error_kind = Some("trace_read_failed".to_string()),
    }
    Ok(report)
}

pub async fn test_hook(
    config: &DaemonConfig,
    hooker_id: &str,
) -> Result<xiaoo_shared::hook_support::HookTestReport> {
    Ok(xiaoo_shared::hook_support::test_hook(&config.app.hooker, hooker_id).await?)
}
