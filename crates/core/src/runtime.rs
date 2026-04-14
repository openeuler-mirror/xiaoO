use std::sync::{Arc, RwLock};

use agent_contracts::context::budget::TokenBudgetPolicy;
use agent_contracts::{CompressionPipeline, PromptBuilder, SkillRegistry, ToolRegistry};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::BuildError;
use llm_client::LlmProviderWrapper;

use crate::snapshot::RuntimeSnapshot;

pub struct RuntimePatch {
    pub llm_provider: Option<Arc<LlmProviderWrapper>>,
    pub tool_registry: Option<Arc<dyn ToolRegistry>>,
    pub skill_registry: Option<Arc<dyn SkillRegistry>>,
    pub prompt_builder: Option<Arc<dyn PromptBuilder>>,
    pub system_prompt: Option<Arc<str>>,
    pub feature_flags: Option<FeatureFlags>,
}

impl Default for RuntimePatch {
    fn default() -> Self {
        Self {
            llm_provider: None,
            tool_registry: None,
            skill_registry: None,
            prompt_builder: None,
            system_prompt: None,
            feature_flags: None,
        }
    }
}

struct Replaceable {
    llm_provider: RwLock<Arc<LlmProviderWrapper>>,
    tool_registry: RwLock<Arc<dyn ToolRegistry>>,
    skill_registry: RwLock<Arc<dyn SkillRegistry>>,
    prompt_builder: RwLock<Arc<dyn PromptBuilder>>,
    system_prompt: RwLock<Arc<str>>,
    feature_flags: RwLock<FeatureFlags>,
}

struct Frozen {
    compression_pipeline: Arc<dyn CompressionPipeline>,
    max_turns: u32,
    token_budget_config: TokenBudgetConfig,
    token_budget_policy: Arc<dyn TokenBudgetPolicy>,
}

pub struct AgentRuntime {
    replaceable: Replaceable,
    frozen: Frozen,
}

impl AgentRuntime {
    pub fn builder() -> AgentRuntimeBuilder {
        AgentRuntimeBuilder::new()
    }

    pub fn set_llm_provider(&self, provider: Arc<LlmProviderWrapper>) {
        *self.replaceable.llm_provider.write().unwrap() = provider;
    }

    pub fn set_tool_registry(&self, registry: Arc<dyn ToolRegistry>) {
        *self.replaceable.tool_registry.write().unwrap() = registry;
    }

    pub fn set_skill_registry(&self, registry: Arc<dyn SkillRegistry>) {
        *self.replaceable.skill_registry.write().unwrap() = registry;
    }

    pub fn set_prompt_builder(&self, builder: Arc<dyn PromptBuilder>) {
        *self.replaceable.prompt_builder.write().unwrap() = builder;
    }

    pub fn set_system_prompt(&self, prompt: Arc<str>) {
        *self.replaceable.system_prompt.write().unwrap() = prompt;
    }

    pub fn set_feature_flags(&self, flags: FeatureFlags) {
        *self.replaceable.feature_flags.write().unwrap() = flags;
    }

    pub fn replace_all(&self, patch: RuntimePatch) {
        if let Some(p) = patch.llm_provider {
            *self.replaceable.llm_provider.write().unwrap() = p;
        }
        if let Some(r) = patch.tool_registry {
            *self.replaceable.tool_registry.write().unwrap() = r;
        }
        if let Some(r) = patch.skill_registry {
            *self.replaceable.skill_registry.write().unwrap() = r;
        }
        if let Some(b) = patch.prompt_builder {
            *self.replaceable.prompt_builder.write().unwrap() = b;
        }
        if let Some(s) = patch.system_prompt {
            *self.replaceable.system_prompt.write().unwrap() = s;
        }
        if let Some(f) = patch.feature_flags {
            *self.replaceable.feature_flags.write().unwrap() = f;
        }
    }

    pub fn llm_provider(&self) -> Arc<LlmProviderWrapper> {
        Arc::clone(&self.replaceable.llm_provider.read().unwrap())
    }

    pub fn tool_registry(&self) -> Arc<dyn ToolRegistry> {
        Arc::clone(&self.replaceable.tool_registry.read().unwrap())
    }

    pub fn skill_registry(&self) -> Arc<dyn SkillRegistry> {
        Arc::clone(&self.replaceable.skill_registry.read().unwrap())
    }

    pub fn prompt_builder(&self) -> Arc<dyn PromptBuilder> {
        Arc::clone(&self.replaceable.prompt_builder.read().unwrap())
    }

    pub fn system_prompt(&self) -> Arc<str> {
        Arc::clone(&self.replaceable.system_prompt.read().unwrap())
    }

    pub fn feature_flags(&self) -> FeatureFlags {
        self.replaceable.feature_flags.read().unwrap().clone()
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            llm_provider: self.llm_provider(),
            tool_registry: self.tool_registry(),
            skill_registry: self.skill_registry(),
            prompt_builder: self.prompt_builder(),
            system_prompt: self.system_prompt(),
            feature_flags: self.feature_flags(),
            compression_pipeline: Arc::clone(&self.frozen.compression_pipeline),
            max_turns: self.frozen.max_turns,
            token_budget_config: self.frozen.token_budget_config.clone(),
            token_budget_policy: Arc::clone(&self.frozen.token_budget_policy),
        }
    }
}

pub struct AgentRuntimeBuilder {
    llm_provider: Option<Arc<LlmProviderWrapper>>,
    compression_pipeline: Option<Arc<dyn CompressionPipeline>>,
    prompt_builder: Option<Arc<dyn PromptBuilder>>,
    system_prompt: Option<Arc<str>>,
    tool_registry: Option<Arc<dyn ToolRegistry>>,
    skill_registry: Option<Arc<dyn SkillRegistry>>,
    feature_flags: Option<FeatureFlags>,
    max_turns: Option<u32>,
    token_budget_config: Option<TokenBudgetConfig>,
    token_budget_policy: Option<Arc<dyn TokenBudgetPolicy>>,
}

impl AgentRuntimeBuilder {
    pub fn new() -> Self {
        Self {
            llm_provider: None,
            compression_pipeline: None,
            prompt_builder: None,
            system_prompt: None,
            tool_registry: None,
            skill_registry: None,
            feature_flags: None,
            max_turns: None,
            token_budget_config: None,
            token_budget_policy: None,
        }
    }

    pub fn llm_provider(mut self, p: Arc<LlmProviderWrapper>) -> Self {
        self.llm_provider = Some(p);
        self
    }

    pub fn compression_pipeline(mut self, p: Arc<dyn CompressionPipeline>) -> Self {
        self.compression_pipeline = Some(p);
        self
    }

    pub fn prompt_builder(mut self, p: Arc<dyn PromptBuilder>) -> Self {
        self.prompt_builder = Some(p);
        self
    }

    pub fn system_prompt(mut self, s: impl Into<Arc<str>>) -> Self {
        self.system_prompt = Some(s.into());
        self
    }

    pub fn tool_registry(mut self, r: Arc<dyn ToolRegistry>) -> Self {
        self.tool_registry = Some(r);
        self
    }

    pub fn skill_registry(mut self, r: Arc<dyn SkillRegistry>) -> Self {
        self.skill_registry = Some(r);
        self
    }

    pub fn feature_flags(mut self, f: FeatureFlags) -> Self {
        self.feature_flags = Some(f);
        self
    }

    pub fn max_turns(mut self, n: u32) -> Self {
        self.max_turns = Some(n);
        self
    }

    pub fn token_budget_config(mut self, c: TokenBudgetConfig) -> Self {
        self.token_budget_config = Some(c);
        self
    }

    pub fn token_budget_policy(mut self, p: Arc<dyn TokenBudgetPolicy>) -> Self {
        self.token_budget_policy = Some(p);
        self
    }

    pub fn build(self) -> Result<AgentRuntime, BuildError> {
        let llm_provider = self
            .llm_provider
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "llm_provider".into(),
            })?;
        let compression_pipeline =
            self.compression_pipeline
                .ok_or_else(|| BuildError::MissingRequiredField {
                    field: "compression_pipeline".into(),
                })?;
        let prompt_builder =
            self.prompt_builder
                .ok_or_else(|| BuildError::MissingRequiredField {
                    field: "prompt_builder".into(),
                })?;
        let system_prompt = self
            .system_prompt
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "system_prompt".into(),
            })?;
        let token_budget_config =
            self.token_budget_config
                .ok_or_else(|| BuildError::MissingRequiredField {
                    field: "token_budget_config".into(),
                })?;
        let token_budget_policy =
            self.token_budget_policy
                .ok_or_else(|| BuildError::MissingRequiredField {
                    field: "token_budget_policy".into(),
                })?;
        let tool_registry = self
            .tool_registry
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "tool_registry".into(),
            })?;
        let skill_registry =
            self.skill_registry
                .ok_or_else(|| BuildError::MissingRequiredField {
                    field: "skill_registry".into(),
                })?;

        let feature_flags = self.feature_flags.unwrap_or_default();
        let max_turns = self.max_turns.unwrap_or(DEFAULT_MAX_TURNS);

        Ok(AgentRuntime {
            replaceable: Replaceable {
                llm_provider: RwLock::new(llm_provider),
                tool_registry: RwLock::new(tool_registry),
                skill_registry: RwLock::new(skill_registry),
                prompt_builder: RwLock::new(prompt_builder),
                system_prompt: RwLock::new(system_prompt),
                feature_flags: RwLock::new(feature_flags),
            },
            frozen: Frozen {
                compression_pipeline,
                max_turns,
                token_budget_config,
                token_budget_policy,
            },
        })
    }
}

const DEFAULT_MAX_TURNS: u32 = 128;
