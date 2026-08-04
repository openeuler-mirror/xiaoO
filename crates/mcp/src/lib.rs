//! Model Context Protocol (MCP) client for xiaoO.
//!
//! Provides a JSON-RPC 2.0 client that speaks the MCP standard over stdio or
//! SSE transports. Each connected MCP server exposes a set of tools which are
//! surfaced to the xiaoO agent runtime through the `ToolSource` extension point
//! (see `crates/tool/src/impl/mcp`).

mod client;
mod config;
mod error;
mod json_config;
mod transport;
mod types;

pub use client::{McpCallResult, McpClient};
pub use config::{EffectSection, McpSection, McpServerConfig, Transport};
pub use error::McpError;
pub use json_config::{
    load_json_servers, merge_server_configs, parse_mcp_json, resolve_json_config_path,
    McpConfigError,
};
pub use transport::McpTransport;
pub use types::McpToolDef;

/// A connected MCP server together with the tools it exposed.
#[derive(Clone)]
pub struct McpServerWithTools {
    pub client: std::sync::Arc<McpClient>,
    pub tools: Vec<McpToolDef>,
    pub effect: EffectSection,
}

/// Connect to, initialise, and list tools from every enabled MCP server in
/// `config`. Unreachable servers are logged and skipped.
///
/// Servers are initialised in parallel; the returned `Vec` preserves the
/// input order so tool registration stays deterministic.
pub async fn init_mcp_tools(servers: &[McpServerConfig]) -> Vec<McpServerWithTools> {
    // Collect enabled indices up front so the returned Vec preserves input
    // order (JoinSet yields in completion order, not insertion order).
    let enabled_indices: Vec<usize> = servers
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_enabled())
        .map(|(i, _)| i)
        .collect();

    let mut join_set = tokio::task::JoinSet::new();
    for &i in &enabled_indices {
        // Clone the config so the spawned task owns it. Configs are small;
        // the clone cost is negligible vs. the network/process round-trips.
        let server = servers[i].clone();
        join_set.spawn(async move { (i, init_one_server(&server).await) });
    }

    let mut results: Vec<Option<McpServerWithTools>> = (0..servers.len()).map(|_| None).collect();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok((i, Some(srv))) => results[i] = Some(srv),
            Ok((_, None)) => {}
            Err(join_error) => {
                tracing::warn!(error = %join_error, "mcp init task panicked");
            }
        }
    }

    let mut out = Vec::new();
    for i in enabled_indices {
        if let Some(srv) = results[i].take() {
            out.push(srv);
        }
    }
    out
}

/// Connect to, initialise, and list tools from a single MCP server. Returns
/// `None` (after logging) if any step fails so the caller can skip the server.
async fn init_one_server(server: &McpServerConfig) -> Option<McpServerWithTools> {
    let client = match McpClient::connect(server).await {
        Ok(c) => c,
        Err(error) => {
            tracing::warn!(
                server = %server.name,
                error = %error,
                "failed to connect mcp server; skipping",
            );
            return None;
        }
    };
    if let Err(error) = client.initialize().await {
        tracing::warn!(
            server = %server.name,
            error = %error,
            "failed to initialize mcp server; skipping",
        );
        return None;
    }
    let client = std::sync::Arc::new(client);
    let tools = match client.list_tools().await {
        Ok(t) => t,
        Err(error) => {
            tracing::warn!(
                server = %server.name,
                error = %error,
                "failed to list mcp tools; skipping server",
            );
            return None;
        }
    };
    tracing::info!(
        server = %server.name,
        count = tools.len(),
        "mcp server connected",
    );
    Some(McpServerWithTools {
        client,
        tools,
        effect: server.effect.clone(),
    })
}
