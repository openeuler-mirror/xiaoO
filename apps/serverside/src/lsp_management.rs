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

pub async fn install_lsp(config: &DaemonConfig, server_id: &str) -> lsp_support::LspActionReport {
    match config.app.lsp.as_ref() {
        Some(lsp) => {
            lsp_support::install_lsp_server(
                lsp.enabled,
                &lsp.disabled_servers,
                &lsp.extra_servers,
                server_id,
            )
            .await
        }
        None => {
            lsp_support::install_lsp_server(
                false,
                &[],
                &[] as &[crate::daemon_config::ExtraServerConfig],
                server_id,
            )
            .await
        }
    }
}

pub async fn test_lsp(
    config: &DaemonConfig,
    server_id: &str,
    workspace: &std::path::Path,
) -> lsp_support::LspActionReport {
    match config.app.lsp.as_ref() {
        Some(lsp) => {
            lsp_support::test_lsp_server(
                lsp.enabled,
                &lsp.disabled_servers,
                &lsp.extra_servers,
                server_id,
                workspace,
            )
            .await
        }
        None => {
            lsp_support::test_lsp_server(
                false,
                &[],
                &[] as &[crate::daemon_config::ExtraServerConfig],
                server_id,
                workspace,
            )
            .await
        }
    }
}
