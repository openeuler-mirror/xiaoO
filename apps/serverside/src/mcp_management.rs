use serde::Serialize;
use std::path::Path;
use xiaoo_shared::mcp_support::{self, McpServerStatus};

use crate::daemon_config::DaemonConfig;

#[derive(Debug, Serialize)]
pub struct McpCatalogReport {
    pub schema_version: u32,
    pub config_path: String,
    pub json_config_path: Option<String>,
    pub servers: Vec<McpServerStatus>,
}

pub async fn mcp_catalog(
    config: &DaemonConfig,
    json_config_path: Option<&Path>,
) -> McpCatalogReport {
    McpCatalogReport {
        schema_version: 1,
        config_path: config.config_path.display().to_string(),
        json_config_path: json_config_path.map(|path| path.display().to_string()),
        servers: mcp_support::inspect_mcp_servers(&config.app.mcp.servers).await,
    }
}
