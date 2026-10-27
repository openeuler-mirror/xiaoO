//! Tool assembly facade.
//!
//! Single implementation behind three public entries
//! ([`discover_tools`] / [`discover_tool_names`] / [`build_tool_registry`])
//! with coarse-grained inputs. Fine-grained types (`ToolSource` /
//! `ToolRuntimeServices` / `ToolRegistryBuilderImpl` / `ToolRegistryConfig` /
//! `ToolVisibilityConfig` / `SubagentRoleConfig`) stay `pub(crate)` and are
//! never exposed to applications: endside consumes `discover_tool_names`,
//! serverside consumes `build_tool_registry`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use agent_contracts::ToolRegistryBuilder;
use agent_types::common::ids::{AgentId, ToolName};
use agent_types::tool::spec_types::EffectProfile;
use agent_types::tool::{ToolRegistryConfig, ToolVisibilityConfig};
use lsp::LspServiceRegistry;
use mcp::McpServerConfig;
use subagent::SubagentControl;
use tool::{
    load_tool_sources_with_services, SubagentRoleConfig, ToolRegistryBuilderImpl,
    ToolRuntimeServices,
};
use xiaoo_api::tools::ToolRegistry;

/// Shared-owned subagent role description (in place of exposing
/// `tool::SubagentRoleConfig`).
///
/// All fields are plain types / plain collections, so apps can fill them
/// straight from their own config structures without `use`-ing any
/// low-level crate. `tools` is a `BTreeMap<String, bool>` carrying the same
/// enable-by-name semantics as `tool::SubagentRoleConfig`.
#[derive(Clone, Default)]
pub struct SubagentRoleSpec {
    pub description: String,
    pub prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub tools: BTreeMap<String, bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCatalogSource {
    Builtin,
    Custom,
    Mcp,
}

impl ToolCatalogSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Custom => "custom",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    pub source: ToolCatalogSource,
    pub effect: EffectProfile,
}

/// Opaque store of bound control handles: one control binding (injected
/// once at startup by `AppBootstrap`) is reused across tool assemblies.
/// The low-level control traits appear only in this struct's internal
/// method signatures — apps hold [`BoundControlStore`] and pass it around,
/// never `use`-ing a trait name.
#[derive(Clone, Default)]
pub struct BoundControlStore {
    inner: Arc<std::sync::RwLock<Option<Arc<dyn SubagentControl>>>>,
}

impl BoundControlStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject the control handles (called once at startup by
    /// `AppBootstrap`).
    pub fn set(&self, control: Arc<dyn SubagentControl>) {
        *self
            .inner
            .write()
            .expect("bound control store lock should not be poisoned") = Some(control);
    }

    /// Current bound control handles, or `None` if not yet bound.
    pub fn snapshot(&self) -> Option<Arc<dyn SubagentControl>> {
        self.inner
            .read()
            .expect("bound control store lock should not be poisoned")
            .clone()
    }
}

/// Opaque MCP tool cache: holds initialized (connect + initialize +
/// list_tools) MCP server connections so multiple tool assemblies share one
/// initialization pass.
///
/// `McpServerWithTools` is a low-level handle that appears only in this
/// struct's internals — apps hold [`McpToolCache`] without `use`-ing any
/// low-level crate.
#[derive(Clone, Default)]
pub struct McpToolCache {
    inner: Arc<tokio::sync::Mutex<Option<Vec<mcp::McpServerWithTools>>>>,
}

impl McpToolCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialized tools, or `None` if not yet initialized. Read-only;
    /// never touches the network.
    pub async fn snapshot(&self) -> Option<Vec<mcp::McpServerWithTools>> {
        self.inner.lock().await.clone()
    }

    /// Initialize with `configs` if not yet initialized, then return a
    /// clone of the tool list. Lazy-init semantics match
    /// [`hosted_runtime_resolver`](super::hosted_runtime_resolver): an empty
    /// config caches an empty list, so "empty" is never mistaken for
    /// "not initialized".
    async fn get_or_init(
        &self,
        configs: &[McpServerConfig],
    ) -> Result<Vec<mcp::McpServerWithTools>, ToolAssemblyError> {
        if let Some(existing) = self.inner.lock().await.clone() {
            return Ok(existing);
        }
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.clone() {
            return Ok(existing);
        }
        let initialised = if configs.is_empty() {
            Vec::new()
        } else {
            mcp::init_mcp_tools(configs).await
        };
        *guard = Some(initialised.clone());
        Ok(initialised)
    }
}

/// Tool assembly input. Fields are plain types plus already-exported shared
/// handles.
///
/// - `lsp_registry`: a config type, held by serverside after config parsing.
/// - `subagent_control`: an opaque handle serverside passes through from
///   shared `CoreBackedSessionService`, no trait import needed.
/// - `mcp_servers`: the MCP config; `mcp_cache`: the cross-assembly
///   initialization cache. When both are given the cache wins; on a cache
///   miss the servers are initialized from `mcp_servers` and written back.
#[derive(Clone, Default)]
pub struct ToolAssemblyInput {
    pub workspace_root: Option<PathBuf>,
    pub disable_plugin_tools: bool,
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    pub mcp_cache: Option<McpToolCache>,
    pub subagent_roles: BTreeMap<String, SubagentRoleSpec>,
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
}

/// Errors from tool assembly.
#[derive(Debug, thiserror::Error)]
pub enum ToolAssemblyError {
    #[error("failed to initialise MCP servers: {0}")]
    McpInit(String),
    #[error("failed to build tool registry: {0}")]
    Build(String),
}

/// Translate assembly input into the low-level `ToolRuntimeServices`
/// (internal, not exported).
fn to_runtime_services(
    input: &ToolAssemblyInput,
    mcp_servers: Option<Vec<mcp::McpServerWithTools>>,
) -> ToolRuntimeServices {
    let subagent_roles = input
        .subagent_roles
        .iter()
        .map(|(role_id, spec)| {
            (
                role_id.clone(),
                SubagentRoleConfig {
                    description: spec.description.clone(),
                    prompt: spec.prompt.clone(),
                    max_turns: spec.max_turns,
                    tools: spec.tools.clone(),
                },
            )
        })
        .collect();
    ToolRuntimeServices {
        disable_plugin_tools: input.disable_plugin_tools,
        subagent_control: input.subagent_control.clone(),
        lsp_registry: input.lsp_registry.clone(),
        workspace_root: input.workspace_root.clone(),
        subagent_roles,
        mcp_servers,
    }
}

/// Resolve the MCP tools this assembly needs: prefer `input.mcp_cache`
/// (shared initialization across assemblies), else initialize directly from
/// `input.mcp_servers` (one-shot, not cached).
///
/// `None` means "no MCP config supplied" (the underlying
/// `ToolRuntimeServices.mcp_servers` stays `None`: no MCP tool source).
/// `Some(vec)` means initialized — even when empty, which stands for "empty
/// config or all servers unreachable", so callers cannot mistake it for
/// "not initialized".
async fn resolve_mcp_tools(
    input: &ToolAssemblyInput,
) -> Result<Option<Vec<mcp::McpServerWithTools>>, ToolAssemblyError> {
    match (&input.mcp_cache, &input.mcp_servers) {
        (Some(cache), Some(configs)) => Ok(Some(cache.get_or_init(configs).await?)),
        (None, Some(configs)) if configs.is_empty() => Ok(Some(Vec::new())),
        (None, Some(configs)) => {
            let initialised = mcp::init_mcp_tools(configs).await;
            Ok(Some(initialised))
        }
        _ => Ok(None),
    }
}

/// Enumerate every tool name discoverable under this assembly input.
///
/// endside's `runtime_request::resolve_visible_tool_names` only needs this:
/// the full tool-name set to intersect with the per-role enable table.
///
/// `async` because an MCP server source must connect + initialize before
/// list_tools (no network when mcp_servers is not supplied).
pub async fn discover_tools(
    input: &ToolAssemblyInput,
) -> Result<Vec<ToolCatalogEntry>, ToolAssemblyError> {
    let mcp_servers = resolve_mcp_tools(input).await?;
    let has_mcp_tools = mcp_servers
        .as_ref()
        .map(|servers| !servers.is_empty())
        .unwrap_or(false);
    let services = to_runtime_services(input, mcp_servers);
    let tool_sources = load_tool_sources_with_services(services);
    let mut source_kinds = vec![ToolCatalogSource::Builtin];
    if !input.disable_plugin_tools {
        source_kinds.push(ToolCatalogSource::Custom);
    }
    if has_mcp_tools {
        source_kinds.push(ToolCatalogSource::Mcp);
    }
    if tool_sources.len() != source_kinds.len() {
        return Err(ToolAssemblyError::Build(
            "tool source catalog classification is inconsistent".to_string(),
        ));
    }

    let mut seen_names = HashSet::new();
    let mut tools = Vec::new();
    for (source, source_kind) in tool_sources.iter().zip(source_kinds) {
        for tool in source.discover() {
            let name = tool.spec.name().0.clone();
            if !seen_names.insert(name.clone()) {
                return Err(ToolAssemblyError::Build(format!(
                    "duplicate tool name in registry: {name}"
                )));
            }
            tools.push(ToolCatalogEntry {
                name,
                description: tool.spec.description().to_string(),
                source: source_kind,
                effect: tool.spec.effect_profile().clone(),
            });
        }
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(tools)
}

pub async fn discover_tool_names(
    input: &ToolAssemblyInput,
) -> Result<Vec<ToolName>, ToolAssemblyError> {
    Ok(discover_tools(input)
        .await?
        .into_iter()
        .map(|tool| ToolName(tool.name))
        .collect())
}

/// Build the tool registry. Visibility filtering is expressed coarsely
/// (per-agent allowed tool names) and translated internally into the
/// low-level `ToolVisibilityConfig`. The returned `Arc<dyn ToolRegistry>`
/// uses the existing `xiaoo_api::tools` export (no C1 violation).
pub async fn build_tool_registry(
    input: ToolAssemblyInput,
    per_agent_allowed_tools: HashMap<AgentId, Vec<ToolName>>,
) -> Result<Arc<dyn ToolRegistry>, ToolAssemblyError> {
    let mcp_servers = resolve_mcp_tools(&input).await?;
    let services = to_runtime_services(&input, mcp_servers);
    let tool_sources = load_tool_sources_with_services(services);

    let registry = ToolRegistryBuilderImpl::new()
        .with_sources(tool_sources)
        .with_config(ToolRegistryConfig {
            visibility: ToolVisibilityConfig {
                per_agent_allowed_tools,
            },
        })
        .build()
        .map_err(|e| ToolAssemblyError::Build(e.to_string()))?;

    Ok(Arc::from(registry))
}

#[cfg(test)]
#[path = "../../../../tests/unit/shared/gateway/tool_assembly_test.rs"]
mod tests;
