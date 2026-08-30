use crate::daemon_config::DaemonConfig;
use anyhow::Result;
use xiaoo_shared::hook_support::HookCatalogReport;

pub fn hook_catalog(config: &DaemonConfig) -> Result<HookCatalogReport> {
    Ok(xiaoo_shared::hook_support::hook_catalog(
        &config.app.hooker,
    )?)
}
