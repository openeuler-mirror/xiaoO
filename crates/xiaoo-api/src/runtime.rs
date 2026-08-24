//! Pure agent runtime SDK.
//!
//! This module deliberately contains no session store, lease, backend pool,
//! gateway bootstrap, or other service-lifecycle machinery.  The caller owns
//! [`RuntimeState`] and injects an operation backend instance when tools need
//! filesystem or process access.
//!
//! # Default dependencies
//!
//! Only [`RuntimeBuilder::llm_provider`] and
//! [`RuntimeBuilder::token_budget_config`] are required; every other runtime
//! dependency falls back to a standard implementation when unset (empty
//! registries, [`prompt::PromptBuilderImpl`], a context manager built from the
//! provider).  The standard implementations are re-exported below so callers
//! can also inject them explicitly.

use std::sync::Arc;

use agent_contracts::{CompressionPipeline, SkillRegistry, TokenBudgetPolicy, ToolRegistry};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use compact::{build_context_manager, CompactionPolicy};
use llm_client::LlmProviderWrapper;
use tool::{EmptyToolRegistry, ToolSpecSnapshot};

use xiaoo_core::{AgentLoopInput, AgentRuntime, LoopRunResult, LoopState};

// ---- 签名可达 re-export（builder/run 签名出现的词汇） ----
#[doc(inline)]
pub use agent_contracts::{PromptBuilder, ToolSpecView};
#[doc(inline)]
pub use agent_types::outcome::AgentError;
#[doc(inline)]
pub use prompt::PromptBuilderImpl;
#[doc(inline)]
pub use xiaoo_core::{spawn_prefetch, LoopStateSnapshot, LoopStopRule, PendingUserMessageSource};

// ---- 挂起/恢复（RuntimeOutput::Suspended 的签名可达词汇） ----

/// Reason a tool call suspended the loop; carried by [`SuspendedToolCall`]
/// inside [`RuntimeOutput`]'s `Suspended` variant.
#[doc(inline)]
pub use xiaoo_core::LoopSuspendReason;
/// One suspended tool call inside [`RuntimeOutput`]'s `Suspended` variant;
/// pairs a `final_call` with the [`LoopSuspendReason`] that paused the loop.
#[doc(inline)]
pub use xiaoo_core::SuspendedToolCall;
/// Build the tool-result [`crate::chat::ChatMessage`] that resumes a
/// suspended loop from a [`agent_types::tool::ToolExecutionResult`].
#[doc(inline)]
pub use xiaoo_core::agent_loop::build_tool_result_message;

// ---- 构建默认件（RuntimeInput.runtime_view 的标准实现） ----

/// Runtime view contract referenced by
/// `RuntimeInput.runtime_view: Option<Arc<dyn RuntimeView>>`; both
/// [`NoopRuntimeView`] and [`BasicRuntimeView`] implement it.
#[doc(inline)]
pub use agent_contracts::runtime::RuntimeView;
/// Default no-op view used when the caller supplies none; the SDK's own
/// [`Runtime::run`] falls back to it.
#[doc(inline)]
pub use xiaoo_core::NoopRuntimeView;
/// Standard runtime view implementation for callers that compose their own
/// view (e.g. wrap it with skill awareness).
#[doc(inline)]
pub use xiaoo_core::BasicRuntimeView;
/// Standard agent context backing [`BasicRuntimeView`]; a building block for
/// callers assembling a custom view.
#[doc(inline)]
pub use xiaoo_core::BasicAgentContext;

use crate::backend::OperationBackend;

/// Mutable, caller-owned state for one conversation.
pub type RuntimeState = LoopState;

/// Input for one runtime turn.
pub type RuntimeInput = AgentLoopInput;

/// Result of one runtime turn.
pub type RuntimeOutput = LoopRunResult;

/// Runtime build error.
///
/// The SDK builder keeps [`RuntimeBuilder::llm_provider`] and
/// [`RuntimeBuilder::token_budget_config`] required (neither has a safe
/// default); every other missing field is filled with a standard
/// implementation at [`RuntimeBuilder::build`] time.  This enum surfaces the
/// two required-field omissions plus the core and compaction failures that can
/// occur while assembling the defaults.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBuildError {
    /// Missing the required LLM provider — the only runtime dependency with no
    /// safe default.
    #[error("missing required llm_provider")]
    MissingLlmProvider,
    /// Missing the required token budget configuration — guessing a budget
    /// risks context overflow or broken compaction, so it stays required.
    #[error("missing required token_budget_config")]
    MissingTokenBudget,
    /// A missing/invalid field reported by the core builder.
    #[error("core runtime build failed: {0}")]
    Core(#[from] agent_types::BuildError),
    /// The default compression pipeline failed to build.
    #[error("default compression pipeline build failed: {0}")]
    Compact(#[from] compact::CompactError),
}

/// Reusable agent decision loop configuration.
pub struct Runtime {
    inner: AgentRuntime,
    operation_backend: Option<Arc<dyn OperationBackend>>,
    default_visible_tools: Vec<Arc<dyn ToolSpecView>>,
}

impl Runtime {
    /// Start configuring a runtime.
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Create caller-owned state for a conversation.
    pub fn new_state(&self, conversation_id: impl Into<String>) -> RuntimeState {
        RuntimeState::new(conversation_id.into())
    }

    /// Run one turn using the supplied state.
    ///
    /// The tool visibility set is decided entirely by `input.visible_tools`:
    /// an empty set means "no tools this turn" (a legal, intentionally-used
    /// semantic in the core loop).  The runtime no longer rewrites an empty
    /// list back to the full registry.  Callers that want the registry's full
    /// tool set must inject it explicitly via [`Runtime::visible_tools`].
    pub async fn run(
        &self,
        state: &mut RuntimeState,
        mut input: RuntimeInput,
    ) -> Result<RuntimeOutput, AgentError> {
        if input.runtime_view.is_none() {
            let view: Arc<dyn RuntimeView> = match input.interaction.clone() {
                Some(interaction) => Arc::new(NoopRuntimeView::with_backend_and_interaction(
                    self.operation_backend.clone(),
                    interaction,
                )),
                None => match &self.operation_backend {
                    Some(backend) => {
                        Arc::new(NoopRuntimeView::with_operation_backend(backend.clone()))
                    }
                    None => Arc::new(NoopRuntimeView::new()),
                },
            };
            if let (Some(backend), Some(interaction)) =
                (&self.operation_backend, input.interaction.clone())
            {
                backend.attach_interaction(interaction);
            }
            input.runtime_view = Some(view);
        }
        xiaoo_core::run_agent_loop(&self.inner, state, input).await
    }

    /// The injected operation backend, if any.
    pub fn operation_backend(&self) -> Option<Arc<dyn OperationBackend>> {
        self.operation_backend.clone()
    }

    /// Return the tool specifications exposed by the configured registry.
    ///
    /// Convenience accessor for callers that want the full registry tool set
    /// as `input.visible_tools` (e.g. via
    /// `RuntimeInput::new(..).with_visible_tools(runtime.visible_tools())`).
    pub fn visible_tools(&self) -> Vec<Arc<dyn ToolSpecView>> {
        self.default_visible_tools.clone()
    }
}

/// Builder for [`Runtime`].
///
/// Holds each field as an `Option`; [`RuntimeBuilder::build`] fills the
/// standard defaults for every unset runtime dependency, then assembles the
/// inner core builder.  Only [`llm_provider`](RuntimeBuilder::llm_provider)
/// and [`token_budget_config`](RuntimeBuilder::token_budget_config) are
/// required.
pub struct RuntimeBuilder {
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
    operation_backend: Option<Arc<dyn OperationBackend>>,
}

impl RuntimeBuilder {
    /// Create an empty builder.
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
            operation_backend: None,
        }
    }

    /// Inject the caller-owned LLM provider (required).
    pub fn llm_provider(mut self, provider: Arc<LlmProviderWrapper>) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// Inject the compression pipeline.
    ///
    /// Unset: a context manager built from the provider via
    /// [`compact::build_context_manager`].
    pub fn compression_pipeline(mut self, pipeline: Arc<dyn CompressionPipeline>) -> Self {
        self.compression_pipeline = Some(pipeline);
        self
    }

    /// Inject the prompt builder.
    ///
    /// Unset: [`PromptBuilderImpl::new`].
    pub fn prompt_builder(mut self, builder: Arc<dyn PromptBuilder>) -> Self {
        self.prompt_builder = Some(builder);
        self
    }

    /// Set the system prompt.
    ///
    /// Unset: an empty string.
    pub fn system_prompt(mut self, prompt: impl Into<Arc<str>>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Inject the tool registry.
    ///
    /// Unset: [`EmptyToolRegistry::new`].
    pub fn tool_registry(mut self, registry: Arc<dyn ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Inject the skill registry.
    ///
    /// Unset: [`xiaoo_core::EmptySkillRegistry::new`].
    pub fn skill_registry(mut self, registry: Arc<dyn SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Set feature flags.
    pub fn feature_flags(mut self, flags: FeatureFlags) -> Self {
        self.feature_flags = Some(flags);
        self
    }

    /// Set the maximum number of turns.
    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Set the token budget configuration (required).
    pub fn token_budget_config(mut self, config: TokenBudgetConfig) -> Self {
        self.token_budget_config = Some(config);
        self
    }

    /// Inject the token budget policy.
    ///
    /// Unset: [`CompactionPolicy::from_budget`] applied to the configured
    /// token budget.
    pub fn token_budget_policy(mut self, policy: Arc<dyn TokenBudgetPolicy>) -> Self {
        self.token_budget_policy = Some(policy);
        self
    }

    /// Inject one caller-owned operation backend instance.
    pub fn operation_backend(mut self, backend: Arc<dyn OperationBackend>) -> Self {
        self.operation_backend = Some(backend);
        self
    }

    /// Build the runtime.
    ///
    /// Required fields (`llm_provider`, `token_budget_config`) must be set;
    /// every other unset dependency is filled with its standard default.
    pub fn build(self) -> Result<Runtime, RuntimeBuildError> {
        let llm_provider = self.llm_provider.ok_or(RuntimeBuildError::MissingLlmProvider)?;
        let token_budget_config = self
            .token_budget_config
            .ok_or(RuntimeBuildError::MissingTokenBudget)?;

        let compression_pipeline = match self.compression_pipeline {
            Some(p) => p,
            None => build_context_manager(None, Arc::clone(&llm_provider))?,
        };
        let token_budget_policy = self.token_budget_policy.unwrap_or_else(|| {
            Arc::new(CompactionPolicy::from_budget(&token_budget_config))
        });
        let prompt_builder = self
            .prompt_builder
            .unwrap_or_else(|| Arc::new(PromptBuilderImpl::new()));
        let tool_registry = self
            .tool_registry
            .unwrap_or_else(|| Arc::new(EmptyToolRegistry::new()));
        let skill_registry = self
            .skill_registry
            .unwrap_or_else(|| Arc::new(xiaoo_core::EmptySkillRegistry::new()));
        let system_prompt = self.system_prompt.unwrap_or_else(|| Arc::from(""));

        let mut inner = AgentRuntime::builder()
            .llm_provider(llm_provider)
            .compression_pipeline(compression_pipeline)
            .prompt_builder(prompt_builder)
            .system_prompt(system_prompt)
            .tool_registry(tool_registry)
            .skill_registry(skill_registry)
            .token_budget_config(token_budget_config)
            .token_budget_policy(token_budget_policy);
        if let Some(flags) = self.feature_flags {
            inner = inner.feature_flags(flags);
        }
        if let Some(max_turns) = self.max_turns {
            inner = inner.max_turns(max_turns);
        }
        let inner = inner.build()?;

        let default_visible_tools = inner
            .tool_registry()
            .list_specs()
            .into_iter()
            .map(|spec| ToolSpecSnapshot::from(spec))
            .map(|spec| Arc::new(spec) as Arc<dyn ToolSpecView>)
            .collect();

        Ok(Runtime {
            inner,
            operation_backend: self.operation_backend,
            default_visible_tools,
        })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use agent_contracts::{LlmProvider, ProviderCapabilities};
    use agent_types::llm::{LlmError, LlmRequest, LlmResponse, StreamChunk};
    use async_trait::async_trait;

    #[test]
    fn runtime_state_is_caller_owned() {
        let state = xiaoo_core::LoopState::new("conversation-1".to_string());
        assert_eq!(state.session_id, "conversation-1");
        assert!(state.messages.read().is_empty());
    }

    #[test]
    fn build_requires_llm_provider() {
        // No provider and no budget -> provider is the first required field
        // resolved in build().  Use `match` (not unwrap_err/expect_err) since
        // Runtime does not implement Debug.
        let err = match Runtime::builder().build() {
            Ok(_) => panic!("build without llm_provider must fail"),
            Err(e) => e,
        };
        assert!(matches!(err, RuntimeBuildError::MissingLlmProvider));
        // Setting budget alone does not satisfy the provider requirement.
        let err = match Runtime::builder()
            .token_budget_config(TokenBudgetConfig {
                total_budget: 8_192,
                reserved_for_output: 1_024,
                reserved_for_system: 512,
                hard_limit_ratio: 0.9,
            })
            .build()
        {
            Ok(_) => panic!("build with budget but no provider must fail"),
            Err(e) => e,
        };
        assert!(matches!(err, RuntimeBuildError::MissingLlmProvider));
    }

    #[test]
    fn build_requires_token_budget() {
        // Provider is set, budget is not — `build()` resolves the provider
        // first, then fails on the missing budget.  The provider here is a
        // stub: `build()` does not invoke `complete`, it only stores the
        // wrapper, so the stub's responses are irrelevant.
        let wrapper = LlmProviderWrapper::new(
            Arc::new(StubLlmProvider::new()) as Arc<dyn LlmProvider>,
            None,
            None,
        );
        let err = match Runtime::builder()
            .llm_provider(Arc::new(wrapper))
            .build()
        {
            Ok(_) => panic!("build with provider but no budget must fail"),
            Err(e) => e,
        };
        assert!(
            matches!(err, RuntimeBuildError::MissingTokenBudget),
            "expected MissingTokenBudget, got {err:?}"
        );
    }

    /// Minimal `LlmProvider` impl whose responses are irrelevant — the build()
    /// path only stores the wrapper, it never calls `complete`.
    struct StubLlmProvider {
        caps: ProviderCapabilities,
    }

    impl StubLlmProvider {
        fn new() -> Self {
            Self {
                caps: ProviderCapabilities {
                    supports_streaming: false,
                    supports_tool_calls: false,
                    supports_json_mode: false,
                    max_context_window: 0,
                    model_name: String::new(),
                },
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StubLlmProvider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            Err(LlmError::ApiError("stub provider".into()))
        }

        async fn complete_stream(
            &self,
            _: &LlmRequest,
            _: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            Err(LlmError::ApiError("stub provider".into()))
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.caps
        }
    }
}
