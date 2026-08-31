use crate::daemon_config::SubagentRoleConfig as ConfigSubagentRole;
use crate::daemon_config::{
    AgentRoleConfig, CompactConfig, DaemonConfig, LlmConfig, ResolvedAgentConfig,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::{fs, path::Path};
use xiaoo_api::chat::{AgentId, FeatureFlags, HookerRegistryConfig, TokenBudgetConfig, ToolName};
use xiaoo_api::llm::LlmProviderWrapper;
use xiaoo_api::llm::{build_context_manager, CompactOverrides, CompressionPipeline};
use xiaoo_api::skills::FileSkillRegistry;
use xiaoo_api::skills::SkillRegistry;
use xiaoo_api::tools::ToolRegistry;
use xiaoo_shared::backend::GatewayBackendConfig;
use xiaoo_shared::gateway::llm_assembly::{build_llm_provider, LlmAssemblyInput};
use xiaoo_shared::gateway::prompt_utils::{
    compose_channel_system_prompt, compose_subagent_delegation_rules, generate_skills_dirs_table,
    ChannelPromptSections,
};
use xiaoo_shared::gateway::session_record::SubagentRoleRecord;
use xiaoo_shared::gateway::tool_assembly::{
    build_tool_registry, discover_tool_names, SubagentRoleSpec, ToolAssemblyInput,
};
use xiaoo_shared::gateway::{
    compose_workspace_system_prompt, ResolvedSessionRuntime, SessionRecord, SessionRuntimeBindings,
    SessionRuntimeBuildInput, SessionRuntimeDescriptor, SessionRuntimeResolveError,
    SessionRuntimeResolver,
};

const DEFAULT_SYSTEM_TOKEN_RESERVE: usize = 2048;
const DEFAULT_MIN_PROMPT_TOKEN_RESERVE: usize = 2048;
const DEFAULT_HARD_LIMIT_RATIO: f64 = 0.8;
const MCP_CHATBOT_INSTANCE_ID: &str = "chatbot";
const MCP_AGENT_INSTANCE_ID: &str = "agent";
const MCP_CHATBOT_SYSTEM_PROMPT: &str = r#"You are xiaoO Chatbot, a direct-answer assistant.

Answer the user's question directly. Do not create plans, delegate work, switch roles, access or modify files, or claim capabilities that are not available. You may search and fetch content from the public web when useful."#;
const MCP_CHATBOT_TOOLS: [&str; 2] = ["web_search", "webfetch"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeCapabilityProfile {
    Default,
    McpChatbot,
    McpAgent,
}

impl RuntimeCapabilityProfile {
    fn from_request(
        request: &SessionRuntimeBuildInput,
    ) -> Result<Self, SessionRuntimeResolveError> {
        if request.entry.kind.as_ref() != Some(&xiaoo_shared::gateway::GatewayEntryKind::Mcp) {
            return Ok(Self::Default);
        }
        match request.entry.instance_id.as_deref() {
            Some(MCP_CHATBOT_INSTANCE_ID) => Ok(Self::McpChatbot),
            Some(MCP_AGENT_INSTANCE_ID) => Ok(Self::McpAgent),
            other => Err(SessionRuntimeResolveError::ResolveFailed {
                message: format!("unknown MCP runtime profile: {other:?}"),
            }),
        }
    }
}

struct EffectiveLlmConfig {
    provider: String,
    model: String,
    api_base: Option<String>,
    api_key_env: Option<String>,
    api_key: Option<String>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct ProviderConfigSig {
    provider: String,
    model: String,
    api_base: Option<String>,
    api_key_env: Option<String>,
    api_key: Option<String>,
}

struct CachedProvider {
    provider: Arc<LlmProviderWrapper>,
    context_window: usize,
}

impl EffectiveLlmConfig {
    fn session_config(&self) -> xiaoo_shared::gateway::LlmRuntimeConfig {
        xiaoo_shared::gateway::LlmRuntimeConfig {
            provider: Some(self.provider.clone()),
            model: Some(self.model.clone()),
            api_base: self.api_base.clone(),
            api_key_env: self.api_key_env.clone(),
            api_key: None,
        }
    }

    fn signature(&self) -> ProviderConfigSig {
        ProviderConfigSig {
            provider: self.provider.clone(),
            model: self.model.clone(),
            api_base: self.api_base.clone(),
            api_key_env: self.api_key_env.clone(),
            api_key: self.api_key.clone(),
        }
    }

    /// Build the shared coarse-grained LLM assembly input from this config.
    /// `context_window_override` is `usize` in the local config but the shared
    /// facade speaks `u32` (model context windows are far below `u32::MAX`).
    fn to_assembly_input(
        &self,
        context_window_override: Option<usize>,
        agent_id: Option<AgentId>,
    ) -> LlmAssemblyInput {
        LlmAssemblyInput {
            provider: self.provider.clone(),
            model: self.model.clone(),
            api_base: self.api_base.clone(),
            api_key: self.api_key.clone(),
            api_key_env: self.api_key_env.clone(),
            context_window_override: context_window_override.and_then(|w| u32::try_from(w).ok()),
            agent_id,
        }
    }
}

struct ResolvedLlmRuntime {
    model: String,
    llm_config: xiaoo_shared::gateway::LlmRuntimeConfig,
    llm_provider: Arc<LlmProviderWrapper>,
    token_budget: TokenBudgetConfig,
    compression_pipeline: Option<Arc<dyn CompressionPipeline>>,
}

pub struct ConfiguredRuntimeResolver {
    agent: ResolvedAgentConfig,
    llm: LlmConfig,
    config_path: PathBuf,
    max_output_tokens: usize,
    effective_context_window: usize,
    llm_provider: Arc<LlmProviderWrapper>,
    provider_pool: Arc<RwLock<HashMap<ProviderConfigSig, CachedProvider>>>,
    compact: Option<CompactConfig>,
    agent_roles: BTreeMap<String, AgentRoleConfig>,
    subagent_roles: BTreeMap<String, ConfigSubagentRole>,
    feature_flags: FeatureFlags,
    trace: Value,
    hooker: HookerRegistryConfig,
    skill_registry: Arc<dyn SkillRegistry>,
    skills_config: xiaoo_shared::skills_support::SkillsConfig,
    skills_dirs: Vec<PathBuf>,
    lsp_registry: Option<Arc<xiaoo_shared::lsp_support::LspServiceRegistry>>,
    operation_backend: Option<GatewayBackendConfig>,
    mcp_servers: Vec<xiaoo_shared::mcp_support::McpServerConfig>,
    /// Lazily initialised MCP server connections; shared across all resolves
    /// so the (expensive) connect + initialize + list_tools runs at most once.
    mcp_tools: xiaoo_shared::gateway::McpToolCache,
    /// Opaque bound-control handle store; populated once at bootstrap by
    /// `AppBootstrap` (via `bound_control_store`) so tool assembly can
    /// inject the bound control into every `ToolRuntimeServices` it builds
    /// without this resolver naming the control trait.
    bound_control: xiaoo_shared::gateway::BoundControlStore,
}

impl ConfiguredRuntimeResolver {
    pub async fn from_config(config: &DaemonConfig) -> Result<Self> {
        let agent = config.resolve_agent()?;
        ensure_workspace_exists(&agent.workspace_root)?;

        let startup_llm = EffectiveLlmConfig {
            provider: config.app.llm.provider.clone(),
            model: agent.model.clone(),
            api_base: config.app.llm.api_base.clone(),
            api_key_env: config.app.llm.api_key_env.clone(),
            api_key: None,
        };
        let (llm_provider, context_window) = build_llm_provider(startup_llm.to_assembly_input(
            config.app.llm.context_window,
            Some(AgentId(agent.id.clone())),
        ))
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .context("failed to resolve llm provider config")?;
        let effective_context_window = context_window as usize;
        let token_budget = build_token_budget(effective_context_window, config.max_output_tokens());

        validate_token_budget_config(
            &token_budget,
            config.max_output_tokens(),
            &agent.model,
            config.config_path(),
        );

        let trace = config.resolve_trace_config();
        let skills_config = config.resolve_skills_config();
        let skill_registry: Arc<dyn SkillRegistry> =
            Arc::new(FileSkillRegistry::new(&skills_config));

        let lsp_registry = config.build_lsp_registry();

        Ok(Self {
            agent,
            llm: config.app.llm.clone(),
            config_path: config.config_path().to_path_buf(),
            max_output_tokens: config.max_output_tokens(),
            effective_context_window,
            llm_provider,
            provider_pool: Arc::new(RwLock::new(HashMap::new())),
            compact: config.resolve_compact_config().cloned(),
            agent_roles: config.app.agent.clone(),
            subagent_roles: config.app.subagent.clone(),
            feature_flags: {
                let mut flags = FeatureFlags::default();
                flags.kvcache_enabled = config.app.llm.kvcache_enabled.unwrap_or(false);
                flags.kvcache_debug_enabled = config.app.llm.kvcache_debug_enabled.unwrap_or(false);
                flags
            },
            trace,
            hooker: config.app.hooker.clone(),
            skill_registry,
            skills_config: skills_config.clone(),
            skills_dirs: skills_config.skills_dirs.clone(),
            operation_backend: config.server_operation_backend(),
            lsp_registry,
            mcp_servers: config.app.mcp.servers.clone(),
            mcp_tools: xiaoo_shared::gateway::McpToolCache::new(),
            bound_control: xiaoo_shared::gateway::BoundControlStore::new(),
        })
    }

    async fn resolve_llm_runtime(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<ResolvedLlmRuntime, SessionRuntimeResolveError> {
        let effective = self.effective_llm_config(request, existing);

        let (llm_provider, effective_context_window) =
            if self.effective_config_matches_startup(&effective) {
                (
                    Arc::clone(&self.llm_provider),
                    self.effective_context_window,
                )
            } else {
                self.resolve_override_provider(&effective).await?
            };
        let token_budget = build_token_budget(effective_context_window, self.max_output_tokens);

        validate_token_budget_config(
            &token_budget,
            self.max_output_tokens,
            &effective.model,
            &self.config_path,
        );

        let compression_pipeline = build_compression_pipeline(self.compact.as_ref(), &llm_provider)
            .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
                message: format!("failed to build compression pipeline: {error}"),
            })?;

        let llm_config = effective.session_config();
        Ok(ResolvedLlmRuntime {
            model: effective.model,
            llm_config,
            llm_provider,
            token_budget,
            compression_pipeline: Some(compression_pipeline),
        })
    }

    fn effective_config_matches_startup(&self, effective: &EffectiveLlmConfig) -> bool {
        effective.provider == self.llm.provider
            && effective.model == self.agent.model
            && effective.api_base == self.llm.api_base
            && effective.api_key_env == self.llm.api_key_env
            && effective.api_key.is_none()
    }

    async fn resolve_override_provider(
        &self,
        effective: &EffectiveLlmConfig,
    ) -> Result<(Arc<LlmProviderWrapper>, usize), SessionRuntimeResolveError> {
        let sig = effective.signature();

        if let Ok(guard) = self.provider_pool.read() {
            if let Some(cached) = guard.get(&sig) {
                tracing::debug!(
                    ?sig.provider,
                    ?sig.model,
                    "Reusing cached override LLM provider from pool"
                );
                return Ok((Arc::clone(&cached.provider), cached.context_window));
            }
        }

        let (created, context_window) = build_llm_provider(effective.to_assembly_input(
            self.llm.context_window,
            Some(AgentId(self.agent.id.clone())),
        ))
        .await
        .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
            message: format!("failed to create llm provider: {error}"),
        })?;
        let context_window = context_window as usize;

        if let Ok(mut guard) = self.provider_pool.write() {
            use std::collections::hash_map::Entry;
            let (provider, context_window) = match guard.entry(sig) {
                Entry::Occupied(e) => (Arc::clone(&e.get().provider), e.get().context_window),
                Entry::Vacant(e) => {
                    e.insert(CachedProvider {
                        provider: Arc::clone(&created),
                        context_window,
                    });
                    (Arc::clone(&created), context_window)
                }
            };
            return Ok((provider, context_window));
        }

        Ok((created, context_window))
    }

    fn effective_llm_config(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> EffectiveLlmConfig {
        let override_llm = request.llm.as_ref();
        let existing_llm = existing.and_then(|session| session.runtime.llm.as_ref());
        EffectiveLlmConfig {
            provider: optional_non_empty(override_llm.and_then(|llm| llm.provider.as_ref()))
                .or_else(|| optional_non_empty(existing_llm.and_then(|llm| llm.provider.as_ref())))
                .unwrap_or_else(|| self.llm.provider.clone()),
            model: optional_non_empty(override_llm.and_then(|llm| llm.model.as_ref()))
                .or_else(|| optional_non_empty(existing_llm.and_then(|llm| llm.model.as_ref())))
                .unwrap_or_else(|| self.agent.model.clone()),
            api_base: optional_non_empty(override_llm.and_then(|llm| llm.api_base.as_ref()))
                .or_else(|| optional_non_empty(existing_llm.and_then(|llm| llm.api_base.as_ref())))
                .or_else(|| self.llm.api_base.clone()),
            api_key_env: optional_non_empty(override_llm.and_then(|llm| llm.api_key_env.as_ref()))
                .or_else(|| {
                    optional_non_empty(existing_llm.and_then(|llm| llm.api_key_env.as_ref()))
                })
                .or_else(|| self.llm.api_key_env.clone()),
            api_key: optional_non_empty(override_llm.and_then(|llm| llm.api_key.as_ref())),
        }
    }

    async fn resolve_e2b_bootstrap(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<
        (
            Option<xiaoo_shared::gateway::RuntimeBootstrapBinding>,
            Option<Arc<xiaoo_shared::backend::E2bBootstrapArchive>>,
        ),
        SessionRuntimeResolveError,
    > {
        let canonical_workspace = request
            .workspace
            .as_deref()
            .map(xiaoo_shared::backend::canonicalize_bootstrap_dir)
            .transpose()
            .map_err(map_bootstrap_error)?;
        let canonical_skill_roots = request
            .skills
            .as_ref()
            .map(|roots| {
                roots
                    .iter()
                    .map(|root| xiaoo_shared::backend::canonicalize_bootstrap_dir(root))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(map_bootstrap_error)?;

        if let Some(binding) =
            existing.and_then(|session| session.runtime.bootstrap_binding.as_ref())
        {
            validate_existing_e2b_binding(
                request,
                canonical_workspace.as_ref(),
                canonical_skill_roots.as_ref(),
                binding,
            )?;
            return Ok((Some(binding.clone()), None));
        }

        let workspace = canonical_workspace;
        let skill_roots = canonical_skill_roots.unwrap_or_default();
        let policy = self.skills_config.clone();
        let archive = tokio::task::spawn_blocking(move || {
            xiaoo_shared::backend::build_e2b_bootstrap_archive(workspace, skill_roots, &policy)
        })
        .await
        .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
            message: format!("E2B bootstrap archive task failed: {error}"),
        })?
        .map_err(map_bootstrap_error)?;
        let binding = archive.binding().clone();
        Ok((Some(binding), Some(Arc::new(archive))))
    }

    /// Build the per-session tool registry via the shared coarse-grained
    /// assembly facade. The facade owns the underlying `ToolRuntimeServices`
    /// / source-loading / builder plumbing, so this resolver only supplies
    /// coarse-grained input (workspace, registry handles, role specs, the
    /// bound opaque subagent control, the lazily-cached MCP connections) and
    /// the per-agent allowed-tool allowlist derived from the profile + role.
    async fn build_tool_registry(
        &self,
        profile: RuntimeCapabilityProfile,
        agent_role: Option<&AgentRoleConfig>,
        workspace_root: PathBuf,
        disable_plugin_tools: bool,
        resolved_agent_id: &AgentId,
    ) -> Result<Option<Arc<dyn ToolRegistry>>, SessionRuntimeResolveError> {
        let input = self.tool_assembly_input(profile, workspace_root, disable_plugin_tools);
        // MCP chatbot sessions are scoped to a fixed, minimal tool allowlist
        // and never expose the full toolset, so short-circuit discovery and
        // build the registry straight from the known set (also avoids any
        // MCP initialisation for the chatbot profile).
        let all_tool_names = if profile == RuntimeCapabilityProfile::McpChatbot {
            MCP_CHATBOT_TOOLS
                .iter()
                .map(|name| ToolName(name.to_string()))
                .collect()
        } else {
            discover_tool_names(&input).await.map_err(|error| {
                SessionRuntimeResolveError::ResolveFailed {
                    message: format!("failed to discover tool names: {error}"),
                }
            })?
        };
        let allowed_tool_names =
            resolve_profile_allowed_tool_names(&all_tool_names, profile, agent_role);
        // Insert the resolved agent_id (root or subagent override) so
        // `filter_for(resolved_agent_id)` in `AppRuntimeFactory::build`
        // finds the entry.
        let mut per_agent_allowed_tools = HashMap::new();
        per_agent_allowed_tools.insert(resolved_agent_id.clone(), allowed_tool_names);

        let registry = build_tool_registry(input, per_agent_allowed_tools)
            .await
            .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
                message: format!("failed to build tool registry: {error}"),
            })?;
        Ok(Some(Arc::from(registry)))
    }

    /// Translate this resolver's per-session inputs into the shared
    /// coarse-grained [`ToolAssemblyInput`]. The chatbot profile drops the
    /// LSP registry, subagent roles, and MCP sources to enforce its strict
    /// tool boundary; the agent/default profiles carry the full set.
    fn tool_assembly_input(
        &self,
        profile: RuntimeCapabilityProfile,
        workspace_root: PathBuf,
        disable_plugin_tools: bool,
    ) -> ToolAssemblyInput {
        let is_chatbot = profile == RuntimeCapabilityProfile::McpChatbot;
        let subagent_roles: BTreeMap<String, SubagentRoleSpec> = if is_chatbot {
            BTreeMap::new()
        } else {
            self.subagent_roles
                .iter()
                .map(|(role_id, config)| {
                    (
                        role_id.clone(),
                        SubagentRoleSpec {
                            description: config.description.clone(),
                            prompt: config.prompt.clone(),
                            max_turns: config.max_turns,
                            tools: config.tools.clone(),
                        },
                    )
                })
                .collect()
        };
        ToolAssemblyInput {
            workspace_root: Some(workspace_root),
            disable_plugin_tools: disable_plugin_tools || is_chatbot,
            lsp_registry: (!is_chatbot).then(|| self.lsp_registry.clone()).flatten(),
            // The chatbot profile never wires MCP sources; the agent/default
            // profiles share the lazily-cached, already-initialised set so
            // the (expensive) connect + list_tools runs at most once.
            mcp_servers: if is_chatbot {
                None
            } else if self.mcp_servers.is_empty() {
                Some(Vec::new())
            } else {
                Some(self.mcp_servers.clone())
            },
            mcp_cache: if is_chatbot {
                None
            } else {
                Some(self.mcp_tools.clone())
            },
            subagent_roles,
            subagent_control: self.bound_control.snapshot(),
        }
    }
}

fn map_bootstrap_error(
    error: xiaoo_shared::backend::E2bBootstrapBuildError,
) -> SessionRuntimeResolveError {
    match error {
        xiaoo_shared::backend::E2bBootstrapBuildError::InvalidPath { message } => {
            SessionRuntimeResolveError::InvalidBootstrap { message }
        }
        xiaoo_shared::backend::E2bBootstrapBuildError::SourceChanged { path } => {
            SessionRuntimeResolveError::BootstrapConflict {
                message: format!(
                    "bootstrap source changed while archiving {}; retry the request",
                    path.display()
                ),
            }
        }
        xiaoo_shared::backend::E2bBootstrapBuildError::CapacityExceeded { message } => {
            SessionRuntimeResolveError::BootstrapTooLarge { message }
        }
        xiaoo_shared::backend::E2bBootstrapBuildError::BuildFailed { message } => {
            SessionRuntimeResolveError::ResolveFailed { message }
        }
    }
}

fn validate_existing_e2b_binding(
    request: &SessionRuntimeBuildInput,
    canonical_workspace: Option<&PathBuf>,
    canonical_skill_roots: Option<&Vec<PathBuf>>,
    binding: &xiaoo_shared::gateway::RuntimeBootstrapBinding,
) -> Result<(), SessionRuntimeResolveError> {
    if binding.manifest_version != xiaoo_shared::gateway::E2B_BOOTSTRAP_MANIFEST_VERSION {
        return Err(SessionRuntimeResolveError::BootstrapConflict {
            message: format!(
                "runtime '{}' uses unsupported bootstrap manifest version {}",
                request.session_id, binding.manifest_version
            ),
        });
    }
    if request.workspace.is_some() && canonical_workspace != binding.source_workspace.as_ref() {
        return Err(SessionRuntimeResolveError::BootstrapConflict {
            message: format!(
                "runtime '{}' is already bound to workspace {:?}, requested {:?}",
                request.session_id, binding.source_workspace, canonical_workspace
            ),
        });
    }
    if request.skills.is_some() && canonical_skill_roots != Some(&binding.source_skill_roots) {
        return Err(SessionRuntimeResolveError::BootstrapConflict {
            message: format!(
                "runtime '{}' is already bound to skill roots {:?}, requested {:?}",
                request.session_id, binding.source_skill_roots, canonical_skill_roots
            ),
        });
    }
    Ok(())
}

fn resolve_local_workspace(
    request: &SessionRuntimeBuildInput,
    existing: Option<&SessionRecord>,
    default_workspace: &Path,
) -> Result<PathBuf, SessionRuntimeResolveError> {
    let requested = request
        .workspace
        .as_deref()
        .map(canonicalize_workspace_dir)
        .transpose()?;
    let existing_workspace = existing
        .map(|record| canonicalize_workspace_dir(&record.runtime.workspace_root))
        .transpose()?;

    if let (Some(requested), Some(existing)) = (&requested, &existing_workspace) {
        if requested != existing {
            return Err(SessionRuntimeResolveError::BootstrapConflict {
                message: format!(
                    "runtime '{}' is already bound to workspace {}, requested {}",
                    request.session_id,
                    existing.display(),
                    requested.display()
                ),
            });
        }
    }

    if let Some(requested) = requested {
        return Ok(requested);
    }
    if let Some(existing) = existing_workspace {
        return Ok(existing);
    }
    canonicalize_workspace_dir(default_workspace)
}

fn canonicalize_workspace_dir(path: &Path) -> Result<PathBuf, SessionRuntimeResolveError> {
    let canonical =
        path.canonicalize()
            .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
                message: format!("invalid workspace {}: {error}", path.display()),
            })?;
    if !canonical.is_dir() {
        return Err(SessionRuntimeResolveError::ResolveFailed {
            message: format!("workspace is not a directory: {}", canonical.display()),
        });
    }
    fs::read_dir(&canonical).map_err(|error| SessionRuntimeResolveError::ResolveFailed {
        message: format!("workspace is not readable {}: {error}", canonical.display()),
    })?;
    Ok(canonical)
}

fn optional_non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[async_trait]
impl SessionRuntimeResolver for ConfiguredRuntimeResolver {
    // The bound control arrives via `bound_control_store()` below (the
    // bootstrap writes into the opaque store); the trait's default
    // `bind_subagent_control` no-op is retained so this resolver never
    // names the control trait.

    fn bound_control_store(&self) -> Option<&xiaoo_shared::gateway::BoundControlStore> {
        Some(&self.bound_control)
    }

    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        let profile = RuntimeCapabilityProfile::from_request(request)?;
        let is_subagent = request
            .agent_id_override
            .as_ref()
            .map(|override_id| override_id != &AgentId(self.agent.id.clone()))
            .unwrap_or(false);
        if profile == RuntimeCapabilityProfile::McpChatbot
            && request.entry.runtime_profile_id.is_some()
        {
            return Err(SessionRuntimeResolveError::ResolveFailed {
                message: "MCP chatbot sessions always use the fixed Chatbot role".to_string(),
            });
        }
        let agent_role =
            resolve_profile_agent_role(&self.agent_roles, request, profile, is_subagent)?;
        let system_prompt = if profile == RuntimeCapabilityProfile::McpChatbot {
            MCP_CHATBOT_SYSTEM_PROMPT
        } else {
            agent_role
                .and_then(|role| role.prompt.as_deref())
                .filter(|prompt| !prompt.trim().is_empty())
                .unwrap_or(self.agent.system_prompt.as_str())
        };

        let subagent_roles: BTreeMap<String, SubagentRoleRecord> =
            if profile == RuntimeCapabilityProfile::McpChatbot {
                BTreeMap::new()
            } else {
                self.subagent_roles
                    .iter()
                    .map(|(role_id, config)| {
                        (
                            role_id.clone(),
                            SubagentRoleRecord {
                                role_id: role_id.clone(),
                                description: config.description.clone(),
                                prompt: config.prompt.clone(),
                                max_turns: config.max_turns,
                                tools: config.tools.clone(),
                            },
                        )
                    })
                    .collect()
            };

        let llm_runtime = self.resolve_llm_runtime(request, existing).await?;
        let is_e2b = self
            .operation_backend
            .as_ref()
            .map(|backend| backend.kind == "e2b")
            .unwrap_or(false);
        if profile == RuntimeCapabilityProfile::McpAgent && is_e2b {
            return Err(SessionRuntimeResolveError::ResolveFailed {
                message: "MCP agent sessions require the local operation backend".to_string(),
            });
        }
        let operation_backend = self.operation_backend.clone().map(|backend| {
            if is_e2b {
                force_e2b_remote_roots(backend)
            } else {
                backend
            }
        });
        let (bootstrap_binding, e2b_bootstrap) = if is_e2b {
            self.resolve_e2b_bootstrap(request, existing).await?
        } else {
            (None, None)
        };
        let local_workspace_root = if is_e2b {
            None
        } else {
            Some(resolve_local_workspace(
                request,
                existing,
                &self.agent.workspace_root,
            )?)
        };
        let effective_workspace_root = bootstrap_binding
            .as_ref()
            .map(|binding| binding.remote_workspace_root.clone())
            .or(local_workspace_root.clone())
            .unwrap_or_else(|| self.agent.workspace_root.clone());
        let effective_skill_roots = bootstrap_binding
            .as_ref()
            .map(|binding| binding.remote_skill_roots.clone())
            .unwrap_or_else(|| self.skills_dirs.clone());

        // MCP servers are lazily initialised (connect + initialize +
        // list_tools) at most once inside the shared `McpToolCache`, which
        // `build_tool_registry` consults below. No eager init here keeps
        // resolve() synchronous up to the tool-assembly call.

        // Honor `agent_id_override` so a subagent lane's tool-lifecycle
        // events are scoped with the subagent's id, not the root's.
        let resolved_agent_id = request
            .agent_id_override
            .clone()
            .unwrap_or_else(|| AgentId(self.agent.id.clone()));
        let tool_registry = self
            .build_tool_registry(
                profile,
                agent_role,
                effective_workspace_root.clone(),
                is_e2b,
                &resolved_agent_id,
            )
            .await?;
        Ok(ResolvedSessionRuntime {
            descriptor: SessionRuntimeDescriptor {
                agent_id: resolved_agent_id,
                model: llm_runtime.model.clone(),
                llm: Some(llm_runtime.llm_config.clone()),
                system_prompt: if profile == RuntimeCapabilityProfile::McpChatbot {
                    system_prompt.to_string()
                } else {
                    build_system_prompt(
                        system_prompt,
                        (!is_e2b).then_some(effective_workspace_root.as_path()),
                        request,
                        &subagent_roles,
                        is_subagent,
                        &effective_skill_roots,
                    )
                },
                feature_flags: self.feature_flags.clone(),

                token_budget: llm_runtime.token_budget.clone(),
                workspace_root: effective_workspace_root.clone(),
                max_turns: agent_role.and_then(|role| role.max_turns),
                subagent_roles,
            },
            entry_kind: request.entry.kind.clone(),
            llm_provider: llm_runtime.llm_provider,
            tool_registry,
            skill_registry: if is_e2b || profile == RuntimeCapabilityProfile::McpChatbot {
                Some(Arc::new(FileSkillRegistry::from_skills(Vec::new())))
            } else {
                Some(Arc::clone(&self.skill_registry))
            },
            bindings: SessionRuntimeBindings::default(),
            compression_pipeline: llm_runtime.compression_pipeline,
            trace: self.trace.clone(),
            hooker: if profile == RuntimeCapabilityProfile::McpChatbot {
                HookerRegistryConfig::default()
            } else {
                self.hooker.clone()
            },
            operation_backend,
            backend_workspace_root: bootstrap_binding
                .as_ref()
                .map(|binding| {
                    PathBuf::from(format!("/xiaoo/e2b-bootstrap/{}", binding.content_digest))
                })
                .or(local_workspace_root)
                .unwrap_or_else(|| self.agent.workspace_root.clone()),
            e2b_bootstrap,
            bootstrap_binding,
            e2b_finalized: false,
        })
    }
}

fn force_e2b_remote_roots(mut backend: GatewayBackendConfig) -> GatewayBackendConfig {
    let mut options = backend.options.as_object().cloned().unwrap_or_default();
    for key in [
        "workspaceRoot",
        "remoteWorkspaceRoot",
        "workspace_root",
        "homeDir",
        "home_dir",
    ] {
        options.remove(key);
    }
    options.insert(
        "workspace_root".to_string(),
        Value::String(xiaoo_shared::gateway::E2B_REMOTE_WORKSPACE_ROOT.to_string()),
    );
    options.insert(
        "home_dir".to_string(),
        Value::String("/home/user".to_string()),
    );
    backend.options = Value::Object(options);
    backend
}

fn resolve_agent_role<'a>(
    agent_roles: &'a BTreeMap<String, AgentRoleConfig>,
    request: &SessionRuntimeBuildInput,
) -> Result<Option<&'a AgentRoleConfig>, SessionRuntimeResolveError> {
    let Some(role_id) = request
        .entry
        .runtime_profile_id
        .as_deref()
        .map(str::trim)
        .filter(|role_id| !role_id.is_empty())
    else {
        return Ok(None);
    };

    agent_roles
        .get(role_id)
        .map(Some)
        .ok_or_else(|| SessionRuntimeResolveError::ResolveFailed {
            message: format!("unknown agent role preset: {role_id}"),
        })
}

fn resolve_profile_agent_role<'a>(
    agent_roles: &'a BTreeMap<String, AgentRoleConfig>,
    request: &SessionRuntimeBuildInput,
    profile: RuntimeCapabilityProfile,
    is_subagent: bool,
) -> Result<Option<&'a AgentRoleConfig>, SessionRuntimeResolveError> {
    match profile {
        RuntimeCapabilityProfile::Default => resolve_agent_role(agent_roles, request),
        RuntimeCapabilityProfile::McpAgent if !is_subagent => {
            resolve_agent_role(agent_roles, request)
        }
        RuntimeCapabilityProfile::McpChatbot | RuntimeCapabilityProfile::McpAgent => Ok(None),
    }
}

fn resolve_allowed_tool_names(
    all_tool_names: &[ToolName],
    agent_role: Option<&AgentRoleConfig>,
) -> Vec<ToolName> {
    let Some(agent_role) = agent_role else {
        return all_tool_names.to_vec();
    };
    if agent_role.tools.is_empty() {
        return all_tool_names.to_vec();
    }

    let available_names: BTreeSet<String> =
        all_tool_names.iter().map(|name| name.0.clone()).collect();
    let mut visible_names: BTreeSet<String> = available_names.clone();
    for (configured_name, enabled) in &agent_role.tools {
        if !available_names.contains(configured_name) {
            continue;
        }
        if *enabled {
            visible_names.insert(configured_name.clone());
        } else {
            visible_names.remove(configured_name);
        }
    }

    all_tool_names
        .iter()
        .filter(|tool_name| visible_names.contains(tool_name.0.as_str()))
        .cloned()
        .collect()
}

fn resolve_profile_allowed_tool_names(
    all_tool_names: &[ToolName],
    profile: RuntimeCapabilityProfile,
    agent_role: Option<&AgentRoleConfig>,
) -> Vec<ToolName> {
    match profile {
        RuntimeCapabilityProfile::McpChatbot => all_tool_names
            .iter()
            .filter(|name| MCP_CHATBOT_TOOLS.contains(&name.0.as_str()))
            .cloned()
            .collect(),
        RuntimeCapabilityProfile::McpAgent => {
            resolve_allowed_tool_names(all_tool_names, agent_role)
                .into_iter()
                .filter(|name| !matches!(name.0.as_str(), "ask_user_question" | "send_file"))
                .collect()
        }
        RuntimeCapabilityProfile::Default => resolve_allowed_tool_names(all_tool_names, agent_role),
    }
}

fn build_token_budget(total_budget: usize, configured_output_tokens: usize) -> TokenBudgetConfig {
    let total_budget = total_budget.max(1);
    let reserved_for_system = DEFAULT_SYSTEM_TOKEN_RESERVE.min(total_budget.saturating_sub(1));
    let reserved_for_prompt = DEFAULT_MIN_PROMPT_TOKEN_RESERVE.min(
        total_budget
            .saturating_sub(reserved_for_system)
            .saturating_sub(1),
    );
    let reserved_for_output = configured_output_tokens.min(
        total_budget
            .saturating_sub(reserved_for_system)
            .saturating_sub(reserved_for_prompt),
    );

    TokenBudgetConfig {
        total_budget,
        reserved_for_output,
        reserved_for_system,
        hard_limit_ratio: DEFAULT_HARD_LIMIT_RATIO,
    }
}

fn validate_token_budget_config(
    budget: &TokenBudgetConfig,
    configured_max_tokens: usize,
    model: &str,
    config_path: &Path,
) {
    let max_reasonable_output_ratio = 0.5;
    let max_reasonable_output_tokens =
        (budget.total_budget as f64 * max_reasonable_output_ratio) as usize;

    if configured_max_tokens > max_reasonable_output_tokens {
        let warning_msg = format!(
            "Warning: max_tokens {} exceeds 50% of the estimated context window for model {}. \
            This may limit input space.",
            configured_max_tokens, model
        );
        tracing::warn!("{}", warning_msg);
    }

    if configured_max_tokens >= budget.total_budget {
        let error_msg = format!(
            "Config Error: max_tokens {} is too large relative to the estimated context window. \
            This would leave NO space for input.",
            configured_max_tokens
        );

        eprintln!("{}", error_msg);
        eprintln!();
        eprintln!("Configuration file: {}", config_path.display());
        eprintln!("Suggestions:");
        eprintln!(
            "  - Reduce max_tokens (currently {}) in [app.llm] section",
            configured_max_tokens
        );
        std::process::abort();
    }
}

fn build_system_prompt(
    base_prompt: &str,
    workspace_root: Option<&Path>,
    request: &SessionRuntimeBuildInput,
    subagent_roles: &BTreeMap<String, SubagentRoleRecord>,
    is_subagent: bool,
    skills_dirs: &[PathBuf],
) -> String {
    let mut base_prompt = workspace_root
        .map(|workspace_root| compose_workspace_system_prompt(base_prompt, workspace_root))
        .unwrap_or_else(|| base_prompt.to_string());
    base_prompt = base_prompt.trim().to_string();

    base_prompt = base_prompt.replace(
        "{{skills_dirs_table}}",
        &generate_skills_dirs_table(skills_dirs),
    );

    if !is_subagent {
        if let Some(rules) = compose_subagent_delegation_rules(&subagent_roles) {
            // Insert Subagent Delegation after identity introduction
            // Find the first double newline after the identity line
            if let Some(pos) = base_prompt.find("\n\n") {
                base_prompt.insert_str(pos, &rules);
            } else {
                base_prompt.push_str(&rules);
            }
        }
    }

    let channel_prompt = request.channel.as_deref().map(|channel| {
        let mut prompt = compose_channel_system_prompt(ChannelPromptSections {
            memory_prompt: "",
            identity_prompt: request.channel_identity_prompt.as_deref().unwrap_or(""),
            group_session_context: None,
        });
        prompt.push_str("\n\n## 当前通道");
        prompt.push_str(&format!("\n- 当前 channel: {channel}."));
        prompt.push_str("\n- 回复必须适合企业 IM 场景，保持纯文本、轻格式。");
        prompt
    });

    match (base_prompt.is_empty(), channel_prompt) {
        (true, Some(channel_prompt)) => channel_prompt,
        (false, Some(channel_prompt)) => format!("{base_prompt}\n\n{channel_prompt}"),
        (false, None) => base_prompt,
        (true, None) => String::new(),
    }
}

fn ensure_workspace_exists(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create workspace {}", path.display()))
}

fn build_compression_pipeline(
    compact: Option<&CompactConfig>,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> Result<Arc<dyn CompressionPipeline>> {
    // A missing `[compact]` section MUST NOT silently disable compression.
    // Previously this fell back to `PassthroughCompressionPipeline` (a no-op),
    // which left the agent loop emitting `Pre-check failed` / `context
    // compression triggered` with `removed=0` forever. Now `None` simply
    // means "use the shared defaults", producing a real `ContextManager`
    // — exactly what the local CLI path does.
    let overrides = compact.map(|c| CompactOverrides {
        warning_ratio: c.warning_ratio,
        auto_compact_ratio: c.auto_compact_ratio,
        blocking_ratio: c.blocking_ratio,
        snip_stale_after_ms: c.snip_stale_after_ms,
        snip_preserve_tail: c.snip_preserve_tail,
        collapse_preserve_tail: c.collapse_preserve_tail,
        summary_max_tokens: c.summary_max_tokens,
        summary_preserve_tail: c.summary_preserve_tail,
        summary_llm_max_tokens: c.summary_llm_max_tokens,
    });
    build_context_manager(overrides.as_ref(), Arc::clone(llm_provider))
        .map_err(|e| anyhow::anyhow!("context manager: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{
        build_system_prompt, build_token_budget, force_e2b_remote_roots, resolve_agent_role,
        resolve_allowed_tool_names, resolve_local_workspace, resolve_profile_agent_role,
        resolve_profile_allowed_tool_names, validate_existing_e2b_binding,
        ConfiguredRuntimeResolver, RuntimeCapabilityProfile, MCP_CHATBOT_TOOLS,
    };
    use crate::daemon_config::{AgentRoleConfig, DaemonConfig};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::{tempdir, TempDir};
    use xiaoo_api::chat::{AgentId, ToolName};
    use xiaoo_shared::backend::GatewayBackendConfig;
    use xiaoo_shared::gateway::{
        GatewayEntryContext, SessionRuntimeBuildInput, SessionRuntimeResolveError,
        SessionRuntimeResolver,
    };

    #[test]
    fn token_budget_caps_output_to_preserve_prompt_budget() {
        let budget = build_token_budget(128_000, 150_000);
        assert_eq!(budget.total_budget, 128_000);
        assert_eq!(budget.reserved_for_system, 2_048);
    }

    #[test]
    fn token_budget_with_explicit_window() {
        let budget = build_token_budget(65536, 8192);
        assert_eq!(budget.total_budget, 65536);
        assert_eq!(budget.reserved_for_output, 8192);
        assert_eq!(budget.reserved_for_system, 2048);
    }

    #[test]
    fn build_system_prompt_includes_workspace_agents_before_channel_rules() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "repo rules").unwrap();
        let request = SessionRuntimeBuildInput {
            session_id: "session".to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "sender".to_string(),
            channel: Some("feishu".to_string()),
            channel_instance_id: None,
            channel_identity_prompt: None,
            entry: GatewayEntryContext::channel(None),
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: None,
            workspace: None,
            skills: None,
        };

        let prompt = build_system_prompt(
            "base rules",
            Some(temp.path()),
            &request,
            &BTreeMap::new(),
            false,
            &Vec::new(),
        );

        assert!(prompt.contains("base rules"));
        assert!(prompt.contains("repo rules"));
        assert!(prompt.contains("当前通道"));
        assert!(prompt.find("repo rules").unwrap() < prompt.find("## 当前通道").unwrap());
    }

    #[test]
    fn resolve_allowed_tool_names_requires_exact_tool_names() {
        let all_tool_names = vec![
            ToolName("file_edit".to_string()),
            ToolName("file_write".to_string()),
        ];
        let agent_role = AgentRoleConfig {
            description: String::new(),
            prompt: None,
            max_turns: None,
            tools: BTreeMap::from([
                ("write".to_string(), false),
                ("file_write".to_string(), false),
            ]),
        };

        let allowed = resolve_allowed_tool_names(&all_tool_names, Some(&agent_role));
        let allowed: Vec<_> = allowed.into_iter().map(|tool| tool.0).collect();

        assert!(allowed.contains(&"file_edit".to_string()));
        assert!(!allowed.contains(&"file_write".to_string()));
    }

    #[test]
    fn mcp_profiles_apply_strict_tool_boundaries() {
        let all = [
            "web_search",
            "webfetch",
            "file_read",
            "glob",
            "grep",
            "file_write",
            "bash",
            "skill",
            "spawn_subagent",
            "ask_user_question",
            "send_file",
            "plugin_custom",
        ]
        .into_iter()
        .map(|name| ToolName(name.to_string()))
        .collect::<Vec<_>>();

        let chatbot =
            resolve_profile_allowed_tool_names(&all, RuntimeCapabilityProfile::McpChatbot, None);
        let chatbot = chatbot.into_iter().map(|name| name.0).collect::<Vec<_>>();
        assert_eq!(chatbot, MCP_CHATBOT_TOOLS.map(str::to_string));

        let agent =
            resolve_profile_allowed_tool_names(&all, RuntimeCapabilityProfile::McpAgent, None);
        let agent = agent.into_iter().map(|name| name.0).collect::<Vec<_>>();
        assert!(agent.contains(&"file_write".to_string()));
        assert!(agent.contains(&"spawn_subagent".to_string()));
        assert!(agent.contains(&"plugin_custom".to_string()));
        assert!(!agent.contains(&"ask_user_question".to_string()));
        assert!(!agent.contains(&"send_file".to_string()));

        let restricted_role = AgentRoleConfig {
            description: String::new(),
            prompt: None,
            max_turns: None,
            tools: BTreeMap::from([("spawn_subagent".to_string(), false)]),
        };
        let restricted = resolve_profile_allowed_tool_names(
            &all,
            RuntimeCapabilityProfile::McpAgent,
            Some(&restricted_role),
        )
        .into_iter()
        .map(|name| name.0)
        .collect::<Vec<_>>();
        assert!(!restricted.contains(&"spawn_subagent".to_string()));
        assert!(restricted.contains(&"file_write".to_string()));
        assert!(!restricted.contains(&"ask_user_question".to_string()));
    }

    #[test]
    fn local_runtime_uses_request_workspace() {
        let default_workspace = tempdir().expect("default workspace");
        let requested_workspace = tempdir().expect("requested workspace");
        let request = SessionRuntimeBuildInput {
            session_id: "mcp-agent-session".to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "sender".to_string(),
            channel: None,
            channel_instance_id: None,
            channel_identity_prompt: None,
            entry: GatewayEntryContext {
                kind: Some(xiaoo_shared::gateway::GatewayEntryKind::Mcp),
                instance_id: Some("agent".to_string()),
                runtime_profile_id: None,
                build_tags: Vec::new(),
            },
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: None,
            workspace: Some(requested_workspace.path().to_path_buf()),
            skills: None,
        };

        let resolved = resolve_local_workspace(&request, None, default_workspace.path())
            .expect("local workspace should resolve");
        assert_eq!(
            resolved,
            requested_workspace
                .path()
                .canonicalize()
                .expect("canonical workspace")
        );
    }

    #[test]
    fn resolve_agent_role_uses_runtime_profile_id() {
        let mut agent_roles = BTreeMap::new();
        agent_roles.insert(
            "code-reviewer".to_string(),
            AgentRoleConfig {
                description: "Reviews code".to_string(),
                prompt: Some("You are a code reviewer.".to_string()),
                max_turns: None,
                tools: BTreeMap::new(),
            },
        );
        let request = SessionRuntimeBuildInput {
            session_id: "session".to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "sender".to_string(),
            channel: Some("http".to_string()),
            channel_instance_id: None,
            channel_identity_prompt: None,
            entry: GatewayEntryContext {
                runtime_profile_id: Some("code-reviewer".to_string()),
                ..GatewayEntryContext::channel(None)
            },
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: None,
            workspace: None,
            skills: None,
        };

        let resolved = resolve_agent_role(&agent_roles, &request)
            .expect("agent role should resolve")
            .expect("agent role should exist");
        assert_eq!(resolved.prompt.as_deref(), Some("You are a code reviewer."));
    }

    #[test]
    fn mcp_agent_role_applies_only_to_the_root_lane() {
        let mut agent_roles = BTreeMap::new();
        agent_roles.insert(
            "xuanyuan".to_string(),
            AgentRoleConfig {
                description: "Operations controller".to_string(),
                prompt: Some("Run the delegated operations workflow.".to_string()),
                max_turns: Some(24),
                tools: BTreeMap::new(),
            },
        );
        let request = SessionRuntimeBuildInput {
            session_id: "mcp-agent-session".to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "mcp-user".to_string(),
            channel: None,
            channel_instance_id: None,
            channel_identity_prompt: None,
            entry: GatewayEntryContext {
                kind: Some(xiaoo_shared::gateway::GatewayEntryKind::Mcp),
                instance_id: Some("agent".to_string()),
                runtime_profile_id: Some("xuanyuan".to_string()),
                build_tags: Vec::new(),
            },
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: None,
            workspace: None,
            skills: None,
        };

        let root_role = resolve_profile_agent_role(
            &agent_roles,
            &request,
            RuntimeCapabilityProfile::McpAgent,
            false,
        )
        .expect("MCP root role should resolve")
        .expect("MCP root should use the configured role");
        assert_eq!(
            root_role.prompt.as_deref(),
            Some("Run the delegated operations workflow.")
        );

        let child_role = resolve_profile_agent_role(
            &agent_roles,
            &request,
            RuntimeCapabilityProfile::McpAgent,
            true,
        )
        .expect("MCP child role resolution should succeed");
        assert!(
            child_role.is_none(),
            "subagent lanes must not inherit the root Xuanyuan prompt"
        );
    }

    #[test]
    fn existing_e2b_binding_inherits_omissions_and_rejects_changes() {
        let workspace = PathBuf::from("/host/workspace");
        let skill_root = PathBuf::from("/host/skills");
        let binding = xiaoo_shared::gateway::RuntimeBootstrapBinding {
            source_workspace: Some(workspace.clone()),
            source_skill_roots: vec![skill_root.clone()],
            content_digest: "digest".to_string(),
            remote_workspace_root: PathBuf::from("/home/user/workspace"),
            remote_skill_roots: vec![PathBuf::from("/home/user/.xiaoo/skills/0")],
            skills: Vec::new(),
            manifest_version: xiaoo_shared::gateway::E2B_BOOTSTRAP_MANIFEST_VERSION,
        };
        let mut request = SessionRuntimeBuildInput {
            session_id: "runtime-1".to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "sender".to_string(),
            channel: None,
            channel_instance_id: None,
            channel_identity_prompt: None,
            entry: GatewayEntryContext::default(),
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: None,
            workspace: None,
            skills: None,
        };

        validate_existing_e2b_binding(&request, None, None, &binding)
            .expect("omission inherits binding");

        request.workspace = Some(workspace.clone());
        request.skills = Some(vec![skill_root.clone()]);
        validate_existing_e2b_binding(
            &request,
            Some(&workspace),
            Some(&vec![skill_root]),
            &binding,
        )
        .expect("same canonical binding is accepted");

        let different = PathBuf::from("/host/different");
        assert!(matches!(
            validate_existing_e2b_binding(
                &request,
                Some(&different),
                Some(&binding.source_skill_roots),
                &binding,
            ),
            Err(SessionRuntimeResolveError::BootstrapConflict { .. })
        ));

        request.workspace = None;
        request.skills = Some(Vec::new());
        assert!(matches!(
            validate_existing_e2b_binding(&request, None, Some(&Vec::new()), &binding),
            Err(SessionRuntimeResolveError::BootstrapConflict { .. })
        ));
    }

    #[test]
    fn e2b_api_runtime_forces_fixed_remote_roots() {
        let backend = force_e2b_remote_roots(GatewayBackendConfig::new(
            "e2b",
            serde_json::json!({
                "workspaceRoot": "/configured/workspace",
                "homeDir": "/configured/home",
                "api_key_env": "E2B_API_KEY"
            }),
        ));

        assert_eq!(backend.options["workspace_root"], "/home/user/workspace");
        assert_eq!(backend.options["home_dir"], "/home/user");
        assert!(backend.options.get("workspaceRoot").is_none());
        assert!(backend.options.get("homeDir").is_none());
        assert_eq!(backend.options["api_key_env"], "E2B_API_KEY");
    }

    /// Pins that when `agent_id_override` is set, the resolved
    /// `descriptor.agent_id` equals the override (so
    /// `SharedToolEventSink` scopes subagent tool-lifecycle events
    /// with the subagent's id, not the root's), and the tool registry
    /// exposes visible tools for the subagent's agent_id.
    #[tokio::test]
    async fn resolve_honors_agent_id_override_for_subagent_lanes() {
        let (config, _workspace_temp) = minimal_test_config().await;
        let resolver = ConfiguredRuntimeResolver::from_config(&config)
            .await
            .expect("construct resolver from minimal config");

        let subagent_agent_id = AgentId(uuid::Uuid::new_v4().to_string());

        let request = SessionRuntimeBuildInput {
            session_id: "test-session".to_string(),
            conversation_id: "test-session".to_string(),
            sender_id: "test-user".to_string(),
            channel: None,
            channel_instance_id: None,
            channel_identity_prompt: None,
            entry: GatewayEntryContext::tui(None),
            agent_id_override: Some(subagent_agent_id.clone()),
            max_turns_override: None,
            subagent_role_id: None,
            llm: None,
            workspace: None,
            skills: None,
        };

        let resolved = resolver
            .resolve(&request, None)
            .await
            .expect("resolve should succeed for subagent lane");

        // `descriptor.agent_id` MUST equal the override so
        // `SharedToolEventSink` scopes lifecycle events correctly.
        assert_eq!(
            resolved.descriptor.agent_id, subagent_agent_id,
            "descriptor.agent_id must honor agent_id_override for subagent lanes"
        );

        // The tool registry MUST expose visible tools for the
        // subagent's agent_id.
        let registry = resolved
            .tool_registry
            .as_ref()
            .expect("tool_registry must be Some for a subagent lane");
        let filter = registry.filter_for(&subagent_agent_id);
        let visible = filter.visible_tools();
        assert!(
            !visible.is_empty(),
            "subagent lane must have visible tools after the build_tool_registry fix"
        );
    }

    /// Minimal `DaemonConfig` for resolver tests. Uses the `ollama`
    /// provider with `api_base` pointed at an unreachable loopback port
    /// so the context-window probe fails instantly (no network wait)
    /// and the static fallback kicks in. The agent's `workspace` is
    /// set to a tempdir so the test does not pollute `$HOME/.xiaoo`.
    async fn minimal_test_config() -> (DaemonConfig, TempDir) {
        let temp = tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let config_path = temp.path().join("config.toml");
        let workspace_str = workspace.to_string_lossy().replace('\\', "\\\\");
        std::fs::write(
            &config_path,
            &format!(
                "[llm]\n\
                 provider = \"ollama\"\n\
                 model = \"llama3\"\n\
                 api_base = \"http://127.0.0.1:1\"\n\
                 [[agents.list]]\n\
                 id = \"main\"\n\
                 default = true\n\
                 workspace = \"{workspace_str}\"\n"
            ),
        )
        .expect("write minimal config");
        let config = DaemonConfig::load_from(&config_path).expect("load minimal config");
        (config, temp)
    }
}
