use crate::config::{
    default_timeout_ms, validate_fixed_headers, EffectSection, McpSection, McpServerConfig,
    Transport,
};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MCP_CONFIG_ENV_VAR: &str = "XIAOO_MCP_CONFIG";

#[derive(Debug, Error)]
pub enum McpConfigError {
    #[error("failed to read MCP config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse MCP config {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid MCP server `{name}`: {message}")]
    InvalidServer { name: String, message: String },

    #[error("invalid MCP server `{name}` in {path}: {message}")]
    InvalidServerAt {
        path: String,
        name: String,
        message: String,
    },

    #[error("duplicate MCP server `{name}` appears in both {toml_source} and {json_source}")]
    DuplicateServer {
        name: String,
        toml_source: PathBuf,
        json_source: PathBuf,
    },
}

impl McpConfigError {
    fn with_path(self, path: &str) -> Self {
        match self {
            Self::InvalidServer { name, message } => Self::InvalidServerAt {
                path: path.to_string(),
                name,
                message,
            },
            error => error,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonMcpConfig {
    #[serde(
        rename = "mcpServers",
        default,
        deserialize_with = "deserialize_server_map"
    )]
    servers: BTreeMap<String, JsonMcpServer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonMcpServer {
    #[serde(default)]
    transport: Transport,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    bearer_token_env: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    effect: EffectSection,
}

fn deserialize_server_map<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, JsonMcpServer>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ServerMapVisitor;

    impl<'de> Visitor<'de> for ServerMapVisitor {
        type Value = BTreeMap<String, JsonMcpServer>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object mapping MCP server names to configurations")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut servers = BTreeMap::new();
            while let Some((name, server)) = map.next_entry::<String, JsonMcpServer>()? {
                if servers.insert(name.clone(), server).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate MCP server key `{name}`"
                    )));
                }
            }
            Ok(servers)
        }
    }

    deserializer.deserialize_map(ServerMapVisitor)
}

pub fn parse_mcp_json(content: &str) -> Result<McpSection, McpConfigError> {
    parse_mcp_json_at(content, "<memory>")
}

fn parse_mcp_json_at(content: &str, path: impl Into<String>) -> Result<McpSection, McpConfigError> {
    let path = path.into();
    let raw: JsonMcpConfig =
        serde_json::from_str(content).map_err(|source| McpConfigError::Parse {
            path: path.clone(),
            source,
        })?;
    let servers = raw
        .servers
        .into_iter()
        .map(|(name, server)| convert_server(name, server))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.with_path(&path))?;
    Ok(McpSection { servers })
}

fn convert_server(name: String, raw: JsonMcpServer) -> Result<McpServerConfig, McpConfigError> {
    if name.trim().is_empty() {
        return invalid_server(name, "server name must not be empty");
    }
    if raw.timeout_ms == 0 {
        return invalid_server(name, "timeout_ms must be greater than zero");
    }

    let command = normalize_optional(raw.command);
    let url = normalize_optional(raw.url);
    let bearer_token_env = normalize_optional(raw.bearer_token_env);
    let agent_id = normalize_optional(raw.agent_id);

    match raw.transport {
        Transport::Stdio => {
            if command.is_none() {
                return invalid_server(name, "stdio transport requires `command`");
            }
            if url.is_some() {
                return invalid_server(name, "stdio transport does not accept `url`");
            }
            if bearer_token_env.is_some() || agent_id.is_some() || !raw.headers.is_empty() {
                return invalid_server(
                    name,
                    "stdio transport does not accept HTTP authentication or headers",
                );
            }
        }
        Transport::Sse | Transport::StreamableHttp => {
            let Some(value) = url.as_deref() else {
                return invalid_server(name, "HTTP transport requires `url`");
            };
            validate_http_url(&name, value)?;
            if command.is_some() || !raw.args.is_empty() || !raw.env.is_empty() {
                return invalid_server(
                    name,
                    "HTTP transport does not accept `command`, `args`, or `env`",
                );
            }
        }
    }

    if let Some(env_name) = bearer_token_env.as_deref() {
        if !is_valid_env_name(env_name) {
            return invalid_server(name, "bearer_token_env must name an environment variable");
        }
    }
    validate_headers(&name, &raw.headers)?;

    Ok(McpServerConfig {
        name,
        transport: raw.transport,
        command,
        args: raw.args,
        env: raw.env,
        url,
        bearer_token_env,
        agent_id,
        headers: raw.headers,
        enabled: raw.enabled,
        timeout_ms: raw.timeout_ms,
        effect: raw.effect,
    })
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn validate_http_url(name: &str, value: &str) -> Result<(), McpConfigError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| invalid_server_error(name, "url must be a valid HTTP(S) URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(invalid_server_error(
            name,
            "url must be a valid HTTP(S) URL",
        ));
    }
    Ok(())
}

fn validate_headers(
    server_name: &str,
    headers: &BTreeMap<String, String>,
) -> Result<(), McpConfigError> {
    validate_fixed_headers(headers).map_err(|message| invalid_server_error(server_name, message))
}

fn is_valid_env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn invalid_server<T>(name: String, message: impl Into<String>) -> Result<T, McpConfigError> {
    Err(McpConfigError::InvalidServer {
        name,
        message: message.into(),
    })
}

fn invalid_server_error(name: &str, message: impl Into<String>) -> McpConfigError {
    McpConfigError::InvalidServer {
        name: name.to_string(),
        message: message.into(),
    }
}

pub fn resolve_json_config_path(
    explicit_path: Option<&Path>,
    workspace: &Path,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = explicit_path.filter(|path| !path.as_os_str().is_empty()) {
        return Some(path.to_path_buf());
    }

    if let Some(path) = std::env::var_os(MCP_CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Some(path);
    }

    let workspace_path = workspace.join(".mcp.json");
    if workspace_path.exists() {
        return Some(workspace_path);
    }

    home.map(|home| home.join(".config/xiaoo/mcp.json"))
        .filter(|path| path.exists())
}

pub fn load_json_servers(
    explicit_path: Option<&Path>,
    workspace: &Path,
    home: Option<&Path>,
) -> Result<Vec<McpServerConfig>, McpConfigError> {
    let Some(path) = resolve_json_config_path(explicit_path, workspace, home) else {
        return Ok(Vec::new());
    };
    let content = std::fs::read_to_string(&path).map_err(|source| McpConfigError::Read {
        path: path.clone(),
        source,
    })?;
    parse_mcp_json_at(&content, path.display().to_string()).map(|section| section.servers)
}

pub fn merge_server_configs(
    toml_servers: Vec<McpServerConfig>,
    json_servers: Vec<McpServerConfig>,
    toml_source: impl AsRef<Path>,
    json_source: impl AsRef<Path>,
) -> Result<Vec<McpServerConfig>, McpConfigError> {
    let mut names = HashSet::new();
    for server in &toml_servers {
        names.insert(server.name.as_str());
    }
    if let Some(server) = json_servers
        .iter()
        .find(|server| names.contains(server.name.as_str()))
    {
        return Err(McpConfigError::DuplicateServer {
            name: server.name.clone(),
            toml_source: toml_source.as_ref().to_path_buf(),
            json_source: json_source.as_ref().to_path_buf(),
        });
    }

    let mut merged = toml_servers;
    merged.extend(json_servers);
    Ok(merged)
}

#[cfg(test)]
#[path = "../../../tests/unit/mcp/json_config_test.rs"]
mod tests;
