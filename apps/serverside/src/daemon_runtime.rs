use crate::daemon_config::SubagentRoleConfig as ConfigSubagentRole;
use crate::daemon_config::{
    AgentRoleConfig, CompactConfig, DaemonConfig, LlmConfig, ResolvedAgentConfig,
};
use agent_contracts::{CompressionPipeline, SkillRegistry, ToolRegistry, ToolRegistryBuilder};
use agent_types::common::ids::{AgentId, ToolName};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::HookerRegistryConfig;
use agent_types::tool::{ToolRegistryConfig, ToolVisibilityConfig};
use anyhow::{Context, Result};
use async_trait::async_trait;
use compact::{
    ContextManager, ContextManagerConfig, ContextThresholds, MicroCompactionPolicy,
    RoughTokenEstimator, RoughTokenEstimatorConfig, SummaryCompressionBudget,
};
use llm_client::{
    create_llm_provider_from_resolved, resolve_config, resolve_model_context_length,
    resolve_provider_profile, LlmProviderWrapper, ResolveInput,
};
use lsp::LspServiceRegistry;
use prompt::{compose_channel_system_prompt, ChannelPromptSections};
use serde_json::Value;
use skill::FileSkillRegistry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::{fs, path::Path};
use tool::{
    load_tool_sources_with_services, SubagentRoleConfig, ToolRegistryBuilderImpl,
    ToolRuntimeServices,
};
use xiaoo_shared::backend::GatewayBackendConfig;
use xiaoo_shared::gateway::prompt_utils::{
    compose_subagent_delegation_rules, generate_skills_dirs_table,
};
use xiaoo_shared::gateway::session_record::SubagentRoleRecord;
use xiaoo_shared::gateway::{
    compose_workspace_system_prompt, ResolvedSessionRuntime, SessionRecord, SessionRuntimeBindings,
    SessionRuntimeBuildInput, SessionRuntimeDescriptor, SessionRuntimeResolveError,
    SessionRuntimeResolver,
};

const DEFAULT_SYSTEM_TOKEN_RESERVE: usize = 2048;
const DEFAULT_MIN_PROMPT_TOKEN_RESERVE: usize = 2048;
const DEFAULT_HARD_LIMIT_RATIO: f64 = 0.8;

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
    skills_dirs: Vec<PathBuf>,
    lsp_registry: Option<Arc<LspServiceRegistry>>,
    operation_backend: Option<GatewayBackendConfig>,
}

impl ConfiguredRuntimeResolver {
    pub async fn from_config(config: &DaemonConfig) -> Result<Self> {
        let agent = config.resolve_agent()?;
        ensure_workspace_exists(&agent.workspace_root)?;

        let resolved_provider = resolve_config(ResolveInput {
            provider: Some(config.app.llm.provider.clone()),
            protocol: None,
            api_key: None,
            api_key_env: config.app.llm.api_key_env.clone(),
            base_url: config.app.llm.api_base.clone(),
        })
        .context("failed to resolve llm provider config")?;
        let llm_provider = Arc::new(
            create_llm_provider_from_resolved(
                &resolved_provider,
                agent.model.clone(),
                Some(agent.id.clone()),
                None,
            )
            .context("failed to create llm provider")?,
        );
        let effective_context_window = resolve_effective_context_window(
            &resolved_provider,
            &agent.model,
            llm_provider.capabilities().max_context_window,
        )
        .await;
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
            skills_dirs: skills_config.skills_dirs.clone(),
            operation_backend: config.server_operation_backend(),
            lsp_registry,
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

        let api_key = resolve_llm_api_key(effective)?;
        let resolved_provider = resolve_config(ResolveInput {
            provider: Some(effective.provider.clone()),
            protocol: None,
            api_key,
            api_key_env: None,
            base_url: effective.api_base.clone(),
        })
        .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
            message: format!("failed to resolve llm provider config: {error}"),
        })?;

        let created = Arc::new(
            create_llm_provider_from_resolved(
                &resolved_provider,
                effective.model.clone(),
                Some(self.agent.id.clone()),
                None,
            )
            .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
                message: format!("failed to create llm provider: {error}"),
            })?,
        );

        let context_window = resolve_effective_context_window(
            &resolved_provider,
            &effective.model,
            created.capabilities().max_context_window,
        )
        .await;

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

    fn build_tool_registry(
        &self,
        agent_role: Option<&AgentRoleConfig>,
    ) -> Result<Option<Arc<dyn ToolRegistry>>, SessionRuntimeResolveError> {
        let subagent_roles: BTreeMap<String, SubagentRoleConfig> = self
            .subagent_roles
            .iter()
            .map(|(role_id, config)| {
                (
                    role_id.clone(),
                    SubagentRoleConfig {
                        description: config.description.clone(),
                        prompt: config.prompt.clone(),
                        max_turns: config.max_turns,
                        tools: config.tools.clone(),
                    },
                )
            })
            .collect();
        let services = ToolRuntimeServices {
            lsp_registry: self.lsp_registry.clone(),
            workspace_root: Some(self.agent.workspace_root.clone()),
            subagent_roles,
            ..ToolRuntimeServices::default()
        };
        let tool_sources = load_tool_sources_with_services(services);
        let all_tool_names: Vec<ToolName> = tool_sources
            .iter()
            .flat_map(|source| source.discover())
            .map(|tool| tool.spec.name().clone())
            .collect();
        let allowed_tool_names = resolve_allowed_tool_names(&all_tool_names, agent_role);
        let mut per_agent_allowed_tools = HashMap::new();
        per_agent_allowed_tools.insert(AgentId(self.agent.id.clone()), allowed_tool_names);

        let registry = ToolRegistryBuilderImpl::new()
            .with_sources(tool_sources)
            .with_config(ToolRegistryConfig {
                visibility: ToolVisibilityConfig {
                    per_agent_allowed_tools,
                },
            })
            .build()
            .map_err(|error| SessionRuntimeResolveError::ResolveFailed {
                message: format!("failed to build tool registry: {error}"),
            })?;

        Ok(Some(Arc::from(registry)))
    }
}

async fn resolve_effective_context_window(
    resolved_provider: &llm_client::ResolvedConfig,
    model: &str,
    static_fallback: usize,
) -> usize {
    match resolve_model_context_length(resolved_provider, model).await {
        Ok(Some(context_window)) => match usize::try_from(context_window) {
            Ok(value) if value > 0 => {
                tracing::info!(
                    context_window = value,
                    source = "model_catalog",
                    model = %model,
                    "Using context_window from model catalog API"
                );
                return value;
            }
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(
                    model = %model,
                    context_window,
                    "dynamic context window does not fit usize; falling back"
                );
            }
        },
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                model = %model,
                error = %error,
                "failed to dynamically resolve model context window; falling back"
            );
        }
    }

    let fallback = static_fallback.max(1);
    tracing::info!(
        context_window = fallback,
        source = "provider_default",
        model = %model,
        "Using context_window from provider default"
    );
    fallback
}

fn resolve_llm_api_key(
    config: &EffectiveLlmConfig,
) -> Result<Option<String>, SessionRuntimeResolveError> {
    if let Some(api_key) = config.api_key.as_ref() {
        return Ok(Some(api_key.clone()));
    }

    if let Some(env_name) = config.api_key_env.as_deref() {
        return resolve_api_key_env(env_name, true);
    }

    if let Some(env_name) = resolve_provider_profile(&config.provider)
        .and_then(|profile| profile.default_api_key_env.map(str::to_string))
    {
        return resolve_api_key_env(&env_name, false);
    }

    Ok(None)
}

fn resolve_api_key_env(
    env_name: &str,
    fail_when_missing: bool,
) -> Result<Option<String>, SessionRuntimeResolveError> {
    if let Some(api_key) = xiaoo_shared::gateway::get_decrypted_api_key(env_name) {
        if !api_key.trim().is_empty() {
            return Ok(Some(api_key));
        }
    }

    match env::var(env_name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(env::VarError::NotPresent) if fail_when_missing => {
            Err(SessionRuntimeResolveError::ResolveFailed {
                message: format!("missing required API key environment variable: {env_name}"),
            })
        }
        Ok(_) | Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(SessionRuntimeResolveError::ResolveFailed {
            message: format!("API key environment variable is not valid unicode: {env_name}"),
        }),
    }
}

fn optional_non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[async_trait]
impl SessionRuntimeResolver for ConfiguredRuntimeResolver {
    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        let agent_role = resolve_agent_role(&self.agent_roles, request)?;
        let system_prompt = agent_role
            .and_then(|role| role.prompt.as_deref())
            .filter(|prompt| !prompt.trim().is_empty())
            .unwrap_or(self.agent.system_prompt.as_str());

        let subagent_roles: BTreeMap<String, SubagentRoleRecord> = self
            .subagent_roles
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
            .collect();

        let is_subagent = request
            .agent_id_override
            .as_ref()
            .map(|override_id| override_id != &AgentId(self.agent.id.clone()))
            .unwrap_or(false);
        let llm_runtime = self.resolve_llm_runtime(request, existing).await?;

        Ok(ResolvedSessionRuntime {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId(self.agent.id.clone()),
                model: llm_runtime.model.clone(),
                llm: Some(llm_runtime.llm_config.clone()),
                system_prompt: build_system_prompt(
                    system_prompt,
                    &self.agent.workspace_root,
                    request,
                    &subagent_roles,
                    is_subagent,
                    &self.skills_dirs,
                ),
                feature_flags: self.feature_flags.clone(),

                token_budget: llm_runtime.token_budget.clone(),
                workspace_root: self.agent.workspace_root.clone(),
                max_turns: agent_role.and_then(|role| role.max_turns),
                subagent_roles,
            },
            entry_kind: request.entry.kind.clone(),
            llm_provider: llm_runtime.llm_provider,
            tool_registry: self.build_tool_registry(agent_role)?,
            skill_registry: Some(Arc::clone(&self.skill_registry)),
            bindings: SessionRuntimeBindings::default(),
            compression_pipeline: llm_runtime.compression_pipeline,
            trace: self.trace.clone(),
            hooker: self.hooker.clone(),
            operation_backend: self.operation_backend.clone(),
        })
    }
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
            "Warning: max_tokens {} exceeds 50% of context_window ({} * 0.5 = {}) for model {}. \
            This may limit input space.",
            configured_max_tokens, budget.total_budget, max_reasonable_output_tokens, model
        );
        tracing::warn!("{}", warning_msg);
    }

    if configured_max_tokens >= budget.total_budget {
        let error_msg = format!(
            "Config Error: max_tokens {} >= context_window {} (from provider/model). \
            This would leave NO space for input.",
            configured_max_tokens, budget.total_budget
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
    workspace_root: &Path,
    request: &SessionRuntimeBuildInput,
    subagent_roles: &BTreeMap<String, SubagentRoleRecord>,
    is_subagent: bool,
    skills_dirs: &[PathBuf],
) -> String {
    let mut base_prompt = compose_workspace_system_prompt(base_prompt, workspace_root);
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
    let compact = match compact {
        Some(cc) => cc,
        None => {
            return Ok(Arc::from(compact::PassthroughCompressionPipeline::new())
                as Arc<dyn CompressionPipeline>);
        }
    };

    let estimator = Arc::new(
        RoughTokenEstimator::try_new(RoughTokenEstimatorConfig {
            chars_per_token: 4,
            message_overhead_tokens: 4,
            tool_use_overhead_tokens: 8,
            tool_result_overhead_tokens: 8,
            image_block_overhead_tokens: 256,
            document_block_overhead_tokens: 256,
        })
        .map_err(|e| anyhow::anyhow!("token estimator: {e}"))?,
    );
    let cc = compact;
    let context_manager_config = ContextManagerConfig {
        thresholds: ContextThresholds {
            warning_ratio: cc.warning_ratio.unwrap_or(0.6),
            auto_compact_ratio: cc.auto_compact_ratio.unwrap_or(0.75),
            blocking_ratio: cc.blocking_ratio.unwrap_or(0.9),
        },
        micro_policy: MicroCompactionPolicy {
            stale_tool_pair_after_ms: 120_000,
            preserve_recent_messages: 6,
        },
        summary_budget: SummaryCompressionBudget {
            max_summary_tokens: cc.summary_max_tokens.unwrap_or(1024),
            preserve_tail_messages: cc.summary_preserve_tail.unwrap_or(4),
        },
        snip_preserve_tail_messages: cc.snip_preserve_tail.unwrap_or(6),
        collapse_preserve_tail_messages: cc.collapse_preserve_tail.unwrap_or(4),
        session_memory_compaction: None,
        snip_stale_after_ms: cc.snip_stale_after_ms.unwrap_or(3_600_000),
    };
    let compression_pipeline: Arc<dyn CompressionPipeline> = Arc::new(
        ContextManager::new(
            estimator,
            context_manager_config,
            Arc::clone(llm_provider),
            agent_types::CompletionConfig {
                max_tokens: cc.summary_llm_max_tokens.unwrap_or(4096),
                temperature: 0.2,
            },
        )
        .map_err(|e| anyhow::anyhow!("context manager: {e}"))?,
    );
    Ok(compression_pipeline)
}

#[cfg(test)]
mod tests {
    use super::{
        build_system_prompt, build_token_budget, resolve_agent_role, resolve_allowed_tool_names,
    };
    use crate::daemon_config::AgentRoleConfig;
    use agent_types::common::ids::ToolName;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;
    use xiaoo_shared::gateway::{GatewayEntryContext, SessionRuntimeBuildInput};

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
        };

        let prompt = build_system_prompt(
            "base rules",
            temp.path(),
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
        };

        let resolved = resolve_agent_role(&agent_roles, &request)
            .expect("agent role should resolve")
            .expect("agent role should exist");
        assert_eq!(resolved.prompt.as_deref(), Some("You are a code reviewer."));
    }
}
