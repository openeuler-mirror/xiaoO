//! Model Context Protocol (MCP) client for xiaoO.
//!
//! Provides a JSON-RPC 2.0 client that speaks the MCP standard over stdio or
//! SSE transports. Each connected MCP server exposes a set of tools which are
//! surfaced to the xiaoO agent runtime through the `ToolSource` extension point
//! (see `crates/tool/src/impl/mcp`).

mod client;
mod config;
mod error;
mod transport;
mod types;

pub use client::{McpCallResult, McpClient};
pub use config::{EffectSection, McpSection, McpServerConfig, Transport};
pub use error::McpError;
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
pub async fn init_mcp_tools(servers: &[McpServerConfig]) -> Vec<McpServerWithTools> {
    let mut out = Vec::new();
    for server in servers {
        if !server.is_enabled() {
            continue;
        }
        if let Some(srv) = init_one_server(server).await {
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
