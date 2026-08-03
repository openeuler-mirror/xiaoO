use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Top-level `[mcp]` configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpSection {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// A single MCP server entry (`[[mcp.servers]]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Logical name used to namespace exposed tools (`mcp__{name}__{tool}`).
    pub name: String,
    #[serde(default)]
    pub transport: Transport,

    /// Command to run for `transport = "stdio"`.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments for the stdio command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the stdio child process.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// URL for `transport = "sse"`.
    #[serde(default)]
    pub url: Option<String>,

    /// Name of the environment variable containing the bearer token. The
    /// secret itself is resolved by the HTTP transport at runtime.
    #[serde(default)]
    pub bearer_token_env: Option<String>,

    /// Optional agent selector sent by Streamable HTTP transports.
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Non-sensitive, fixed headers for HTTP transports.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    /// Override the enabled flag (defaults to true).
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Request/handshake timeout in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Declared effect profile for this server's tools. Defaults to the most
    /// conservative assumption (all effects present) so batches containing
    /// these tools are serialised; relax it for known read-only servers to
    /// allow parallel execution.
    #[serde(default)]
    pub effect: EffectSection,
}

impl McpServerConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

pub(crate) fn default_timeout_ms() -> u64 {
    30_000
}

pub(crate) fn validate_fixed_headers(headers: &BTreeMap<String, String>) -> Result<(), String> {
    for (name, value) in headers {
        let parsed_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid header name `{name}`"))?;
        let normalized = parsed_name.as_str();
        if is_sensitive_header_name(normalized) {
            return Err(format!(
                "sensitive header `{name}` is not allowed; use bearer_token_env"
            ));
        }
        if matches!(
            normalized,
            "origin"
                | "mcp-session-id"
                | "mcp-protocol-version"
                | "accept"
                | "content-type"
                | "x-agent-id"
                | "last-event-id"
                | "mcp-method"
                | "mcp-name"
        ) {
            return Err(format!("transport-managed header `{name}` is not allowed"));
        }
        reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| format!("invalid value for header `{name}`"))?;
    }
    Ok(())
}

fn is_sensitive_header_name(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) || name.contains("token")
        || name.contains("api-key")
}

/// Effect profile for an MCP server's tools. The MCP protocol does not expose
/// effect metadata in tool definitions, so this is a per-server declaration.
/// Defaults to the most conservative assumption (all effects present); users
/// who know their server's tools are read-only can relax it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectSection {
    #[serde(default = "default_true")]
    pub reads_filesystem: bool,
    #[serde(default = "default_true")]
    pub writes_filesystem: bool,
    #[serde(default = "default_true")]
    pub network_access: bool,
    #[serde(default = "default_true")]
    pub side_effects: bool,
}

impl Default for EffectSection {
    /// Defaults to the most conservative assumption (all effects present) so
    /// batches containing these tools are serialised unless the user relaxes
    /// the profile. This keeps the whole-section-missing path (`#[serde(default)]`
    /// on `McpServerConfig.effect` calls `EffectSection::default()`) consistent
    /// with the per-field path (`#[serde(default = "default_true")]` used when
    /// individual fields are omitted from an explicit `[mcp.servers.effect]`
    /// section). The derived `Default` produced all-`false`, contradicting
    /// both the per-field serde default and the documented "all default to
    /// true" contract.
    fn default() -> Self {
        Self {
            reads_filesystem: true,
            writes_filesystem: true,
            network_access: true,
            side_effects: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Stdio,
    Sse,
    StreamableHttp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_section_default_is_all_true() {
        let e = EffectSection::default();
        assert!(
            e.reads_filesystem,
            "reads_filesystem should default to true"
        );
        assert!(
            e.writes_filesystem,
            "writes_filesystem should default to true"
        );
        assert!(e.network_access, "network_access should default to true");
        assert!(e.side_effects, "side_effects should default to true");
    }

    #[test]
    fn missing_effect_section_defaults_to_all_true() {
        // Reproduces the regression where a server declared without an
        // `[mcp.servers.effect]` section got `EffectSection::default()` from
        // the derived `#[derive(Default)]` (all `false`), contradicting the
        // documented "all four fields default to true" contract. With the
        // manual `Default` impl, both the whole-section-missing path and the
        // per-field-missing path must agree on `true`.
        let toml = r#"
[[servers]]
name = "lookup"
transport = "stdio"
command = "./lookup-server"
"#;
        let mcp: McpSection = toml::from_str(toml).unwrap();
        let effect = &mcp.servers[0].effect;
        assert!(effect.reads_filesystem);
        assert!(effect.writes_filesystem);
        assert!(effect.network_access);
        assert!(effect.side_effects);
    }

    #[test]
    fn partial_effect_section_omits_fields_default_to_true() {
        let toml = r#"
[[servers]]
name = "lookup"
transport = "stdio"
command = "./lookup-server"

[servers.effect]
writes_filesystem = false
side_effects = false
"#;
        let mcp: McpSection = toml::from_str(toml).unwrap();
        let effect = &mcp.servers[0].effect;
        assert!(
            effect.reads_filesystem,
            "omitted field should use default_true"
        );
        assert!(!effect.writes_filesystem);
        assert!(
            effect.network_access,
            "omitted field should use default_true"
        );
        assert!(!effect.side_effects);
    }
}
