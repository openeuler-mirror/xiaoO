use crate::gateway::{
    compose_workspace_system_prompt, ResolvedSessionRuntime, SessionRecord, SessionRuntimeBindings,
    SessionRuntimeBuildInput, SessionRuntimeDescriptor, SessionRuntimeResolveError,
    SessionRuntimeResolver,
};
use agent_contracts::backend::OperationBackendConfig;
use agent_contracts::{CompressionPipeline, SkillRegistry, ToolRegistry, ToolRegistryBuilder};
use agent_types::common::ids::{AgentId, ToolName};
use agent_types::hook::HookerRegistryConfig;
use agent_types::tool::{ToolRegistryConfig, ToolVisibilityConfig};
use async_trait::async_trait;
use llm_client::{create_llm_provider, LlmProviderConfig, LlmProviderWrapper};
use lsp::LspServiceRegistry;
use serde_json::Value;
use skill::{FileSkillRegistry, SkillsConfig};
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, RwLock};
use subagent::SubagentControl;
use tool::{load_tool_sources_with_services, ToolRegistryBuilderImpl, ToolRuntimeServices};

#[derive(Clone)]
pub struct HostedSessionRuntimeConfig {
    pub descriptor: SessionRuntimeDescriptor,
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub api_base: Option<String>,
    pub visible_tool_names: Option<Vec<String>>,
    pub compression_pipeline: Option<Arc<dyn CompressionPipeline>>,
    pub llm_provider: Option<Arc<LlmProviderWrapper>>,
    pub trace: Value,
    pub hooker: HookerRegistryConfig,
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub operation_backend: Option<OperationBackendConfig>,
    pub skills_config: SkillsConfig,
}

pub struct HostedSessionRuntimeResolver {
    config: HostedSessionRuntimeConfig,
    bindings: SessionRuntimeBindings,
    tool_runtime_services: Arc<RwLock<ToolRuntimeServices>>,
}

impl HostedSessionRuntimeResolver {
    pub fn new(config: HostedSessionRuntimeConfig, bindings: SessionRuntimeBindings) -> Self {
        let initial_services = ToolRuntimeServices {
            lsp_registry: config.lsp_registry.clone(),
            workspace_root: Some(config.descriptor.workspace_root.clone()),
            ..ToolRuntimeServices::default()
        };
        Self {
            config,
            bindings,
            tool_runtime_services: Arc::new(RwLock::new(initial_services)),
        }
    }

    fn resolve_api_key(&self) -> Result<Option<String>, SessionRuntimeResolveError> {
        if let Some(api_key) = self.config.api_key.clone() {
            return Ok(Some(api_key));
        }

        let Some(env_name) = self.config.api_key_env.as_deref() else {
            return Ok(None);
        };

        match env::var(env_name) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
            Ok(_) | Err(env::VarError::NotPresent) => {
                Err(SessionRuntimeResolveError::ResolveFailed {
                    message: format!("missing required API key environment variable: {env_name}"),
                })
            }
            Err(env::VarError::NotUnicode(_)) => Err(SessionRuntimeResolveError::ResolveFailed {
                message: format!("API key environment variable is not valid unicode: {env_name}"),
            }),
        }
    }

    fn build_tool_registry(
        &self,
        agent_id: &AgentId,
        services: ToolRuntimeServices,
    ) -> Result<Option<Arc<dyn ToolRegistry>>, SessionRuntimeResolveError> {
        let Some(visible_tool_names) = self.config.visible_tool_names.as_ref() else {
            let tool_sources = load_tool_sources_with_services(services.clone());
            let all_tool_names = tool_sources
                .iter()
                .flat_map(|source| source.discover())
                .map(|tool| tool.spec.name().clone())
                .collect();
            let mut per_agent_allowed_tools = HashMap::new();
            per_agent_allowed_tools.insert(agent_id.clone(), all_tool_names);
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

            return Ok(Some(Arc::from(registry)));
        };

        if visible_tool_names.is_empty() {
            return Ok(None);
        }

        let mut per_agent_allowed_tools = HashMap::new();
        per_agent_allowed_tools.insert(
            agent_id.clone(),
            visible_tool_names.iter().cloned().map(ToolName).collect(),
        );

        let registry = ToolRegistryBuilderImpl::new()
            .with_sources(load_tool_sources_with_services(services))
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

    fn build_skill_registry(skills_config: &SkillsConfig) -> Arc<dyn SkillRegistry> {
        Arc::new(FileSkillRegistry::new(skills_config))
    }
}

#[async_trait]
impl SessionRuntimeResolver for HostedSessionRuntimeResolver {
    fn bind_subagent_control(&self, control: Arc<dyn SubagentControl>) {
        self.tool_runtime_services
            .write()
            .expect("tool runtime services lock should not be poisoned")
            .subagent_control = Some(control);
    }

    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        _existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        let agent_id = request
            .agent_id_override
            .clone()
            .unwrap_or_else(|| self.config.descriptor.agent_id.clone());
        let llm_provider = match self.config.llm_provider.clone() {
            Some(provider) => Arc::new(LlmProviderWrapper::new(
                provider.inner(),
                Some(agent_id.0.clone()),
                None,
            )),
            None => {
                let api_key = self.resolve_api_key()?;
                let llm_config = LlmProviderConfig {
                    provider: self.config.provider.clone(),
                    api_key,
                    api_base: self.config.api_base.clone(),
                    model: self.config.model.clone(),
                };
                Arc::new(
                    create_llm_provider(&llm_config, Some(agent_id.0.clone()), None).map_err(
                        |error| SessionRuntimeResolveError::ResolveFailed {
                            message: format!("failed to create llm provider: {error}"),
                        },
                    )?,
                )
            }
        };
        let mut services = self
            .tool_runtime_services
            .read()
            .expect("tool runtime services lock should not be poisoned")
            .clone();
        services.workspace_root = Some(self.config.descriptor.workspace_root.clone());
        let mut descriptor = self.config.descriptor.clone();
        descriptor.agent_id = agent_id.clone();
        descriptor.system_prompt =
            compose_workspace_system_prompt(&descriptor.system_prompt, &descriptor.workspace_root);

        Ok(ResolvedSessionRuntime {
            descriptor,
            entry_kind: request.entry.kind.clone(),
            llm_provider,
            tool_registry: self.build_tool_registry(&agent_id, services)?,
            skill_registry: Some(Self::build_skill_registry(&self.config.skills_config)),
            bindings: self.bindings.clone(),
            trace: self.config.trace.clone(),
            compression_pipeline: self.config.compression_pipeline.clone(),
            hooker: self.config.hooker.clone(),
            operation_backend: self.config.operation_backend.clone(),
        })
    }
}
