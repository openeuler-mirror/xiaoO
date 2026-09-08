use crate::daemon_config::DaemonConfig;
use xiaoo_shared::lsp_support::{self, LspCatalogReport};

pub fn lsp_catalog(config: &DaemonConfig) -> LspCatalogReport {
    match config.app.lsp.as_ref() {
        Some(lsp) => {
            lsp_support::lsp_catalog(lsp.enabled, &lsp.disabled_servers, &lsp.extra_servers)
        }
        None => lsp_support::lsp_catalog(
            false,
            &[],
            &[] as &[crate::daemon_config::ExtraServerConfig],
        ),
    }
}
