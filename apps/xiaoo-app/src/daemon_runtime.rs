use crate::daemon_config::{AgentRoleConfig, DaemonConfig, ResolvedAgentConfig, SubagentRoleConfig};
use xiaoo_app::gateway::prompt_utils::compose_subagent_delegation_rules;
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
    LlmProviderWrapper, ResolveInput,
};
use lsp::LspServiceRegistry;
use prompt::{compose_channel_system_prompt, ChannelPromptSections};
use serde_json::Value;
use skill::FileSkillRegistry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::{fs, path::Path};
use tool::{load_tool_sources_with_services, ToolRegistryBuilderImpl, ToolRuntimeServices, SubagentRoleInfo};
use xiaoo_app::gateway::{
    backend::GatewayBackendConfig, compose_workspace_system_prompt, ResolvedSessionRuntime,
    SessionRecord, SessionRuntimeBindings, SessionRuntimeBuildInput, SessionRuntimeDescriptor,
    SessionRuntimeResolveError, SessionRuntimeResolver,
};
use xiaoo_app::gateway::session_record::SubagentRoleRecord;

const DEFAULT_SYSTEM_TOKEN_RESERVE: usize = 2048;
const DEFAULT_MIN_PROMPT_TOKEN_RESERVE: usize = 2048;
const DEFAULT_HARD_LIMIT_RATIO: f64 = 0.8;

pub struct ConfiguredRuntimeResolver {
    agent: ResolvedAgentConfig,
    agent_roles: BTreeMap<String, AgentRoleConfig>,
    subagent_roles: BTreeMap<String, SubagentRoleConfig>,
    llm_provider: Arc<LlmProviderWrapper>,
    token_budget: TokenBudgetConfig,
    feature_flags: FeatureFlags,
    trace: Value,
    compression_pipeline: Option<Arc<dyn CompressionPipeline>>,
    hooker: HookerRegistryConfig,
    skill_registry: Arc<dyn SkillRegistry>,
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
            config.app.llm.context_window,
            &resolved_provider,
            &agent.model,
            llm_provider.capabilities().max_context_window,
        )
        .await;
        let token_budget = build_token_budget(
            Some(effective_context_window),
            config.max_output_tokens(),
            llm_provider.capabilities().max_context_window,
        );

        let trace = config.resolve_trace_config();
        let compression_pipeline = build_compression_pipeline(config, &llm_provider)?;
        let skill_registry: Arc<dyn SkillRegistry> =
            Arc::new(FileSkillRegistry::new(&config.resolve_skills_config()));

        let lsp_registry = config.build_lsp_registry();

        Ok(Self {
            agent,
            agent_roles: config.app.agent.clone(),
            subagent_roles: config.app.subagent.clone(),
            llm_provider,
            token_budget,
            feature_flags: {
                let mut flags = FeatureFlags::default();
                flags.kvcache_enabled = config.app.llm.kvcache_enabled.unwrap_or(false);
                flags.kvcache_debug_enabled = config.app.llm.kvcache_debug_enabled.unwrap_or(false);
                flags
            },
            trace,
            compression_pipeline: Some(compression_pipeline),
            hooker: config.app.hooker.clone(),
            skill_registry,
            operation_backend: config.app.operation_backend.clone(),
            lsp_registry,
        })
    }

    fn build_tool_registry(
        &self,
        agent_role: Option<&AgentRoleConfig>,
    ) -> Result<Option<Arc<dyn ToolRegistry>>, SessionRuntimeResolveError> {
        let subagent_roles: BTreeMap<String, SubagentRoleInfo> = self
            .subagent_roles
            .iter()
            .map(|(role_id, config)| {
                (role_id.clone(), SubagentRoleInfo {
                    role_id: role_id.clone(),
                    description: config.description.clone(),
                })
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
    configured_context_window: Option<usize>,
    resolved_provider: &llm_client::ResolvedConfig,
    model: &str,
    static_fallback: usize,
) -> usize {
    if let Some(configured) = configured_context_window.filter(|value| *value > 0) {
        return configured;
    }

    match resolve_model_context_length(resolved_provider, model).await {
        Ok(Some(context_window)) => match usize::try_from(context_window) {
            Ok(value) if value > 0 => return value,
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

    static_fallback.max(1)
}

#[async_trait]
impl SessionRuntimeResolver for ConfiguredRuntimeResolver {
    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        _existing: Option<&SessionRecord>,
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
                (role_id.clone(), SubagentRoleRecord {
                    role_id: role_id.clone(),
                    description: config.description.clone(),
                    prompt: config.prompt.clone(),
                    max_turns: config.max_turns,
                    tools: config.tools.clone(),
                })
            })
            .collect();

        let is_subagent = request.agent_id_override.as_ref()
            .map(|override_id| override_id != &AgentId(self.agent.id.clone()))
            .unwrap_or(false);

        Ok(ResolvedSessionRuntime {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId(self.agent.id.clone()),
                model: self.agent.model.clone(),
                system_prompt: build_system_prompt(
                    system_prompt,
                    &self.agent.workspace_root,
                    request,
                    &subagent_roles,
                    is_subagent,
                ),
                feature_flags: self.feature_flags.clone(),

                token_budget: self.token_budget.clone(),
                workspace_root: self.agent.workspace_root.clone(),
                max_turns: agent_role.and_then(|role| role.max_turns),
                subagent_roles,
            },
            entry_kind: request.entry.kind.clone(),
            llm_provider: Arc::clone(&self.llm_provider),
            tool_registry: self.build_tool_registry(agent_role)?,
            skill_registry: Some(Arc::clone(&self.skill_registry)),
            bindings: SessionRuntimeBindings::default(),
            compression_pipeline: self.compression_pipeline.clone(),
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

fn build_token_budget(
    configured_context_window: Option<usize>,
    configured_output_tokens: usize,
    provider_context_window: usize,
) -> TokenBudgetConfig {
    let total_budget = configured_context_window
        .unwrap_or(provider_context_window)
        .max(1);
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

fn build_system_prompt(
    base_prompt: &str,
    workspace_root: &Path,
    request: &SessionRuntimeBuildInput,
    subagent_roles: &BTreeMap<String, SubagentRoleRecord>,
    is_subagent: bool,
) -> String {
    let mut base_prompt = compose_workspace_system_prompt(base_prompt, workspace_root);
    base_prompt = base_prompt.trim().to_string();

    if !is_subagent {
        if let Some(rules) = compose_subagent_delegation_rules(&subagent_roles) {
            base_prompt.push_str(&rules);
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
    config: &DaemonConfig,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> Result<Arc<dyn CompressionPipeline>> {
    let compact = match config.resolve_compact_config() {
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
    use xiaoo_app::gateway::{GatewayEntryContext, SessionRuntimeBuildInput};

    #[test]
    fn token_budget_caps_output_to_preserve_prompt_budget() {
        let budget = build_token_budget(None, 150_000, 128_000);
        assert_eq!(budget.total_budget, 128_000);
        assert_eq!(budget.reserved_for_output, 123_904);
        assert_eq!(budget.reserved_for_system, 2_048);
    }

    #[test]
    fn token_budget_prefers_configured_context_window() {
        let budget = build_token_budget(Some(65536), 8192, 128000);
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
        };

        let prompt = build_system_prompt("base rules", temp.path(), &request, &BTreeMap::new(), false);

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
        };

        let resolved = resolve_agent_role(&agent_roles, &request)
            .expect("agent role should resolve")
            .expect("agent role should exist");
        assert_eq!(resolved.prompt.as_deref(), Some("You are a code reviewer."));
    }
}
