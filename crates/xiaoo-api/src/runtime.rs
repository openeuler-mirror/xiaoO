//! Pure agent runtime SDK.
//!
//! This module deliberately contains no session store, lease, backend pool,
//! gateway bootstrap, or other service-lifecycle machinery.  The caller owns
//! [`RuntimeState`] and injects an operation backend instance when tools need
//! filesystem or process access.

use std::sync::Arc;

use agent_contracts::{
    CompressionPipeline, PromptBuilder, RuntimeView, SkillRegistry, TokenBudgetPolicy, ToolRegistry,
};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::BuildError;
use llm_client::LlmProviderWrapper;

use xiaoo_core::{
    run_agent_loop, AgentLoopInput, AgentRuntime, AgentRuntimeBuilder, LoopRunResult, LoopState,
    NoopRuntimeView,
};

use crate::backend::OperationBackend;

/// Mutable, caller-owned state for one conversation.
pub type RuntimeState = LoopState;

/// Input for one runtime turn.
pub type RuntimeInput = AgentLoopInput;

/// Result of one runtime turn.
pub type RuntimeOutput = LoopRunResult;

/// A narrow extension point for integrations that need to decorate the
/// runtime view.  It intentionally cannot access gateway/session services.
pub trait RuntimeExtension: Send + Sync {
    /// Wrap the runtime view used by a turn.
    fn wrap_runtime_view(&self, view: Arc<dyn RuntimeView>) -> Arc<dyn RuntimeView>;
}

/// Reusable agent decision loop configuration.
pub struct Runtime {
    inner: AgentRuntime,
    operation_backend: Option<Arc<dyn OperationBackend>>,
    extensions: Vec<Arc<dyn RuntimeExtension>>,
    default_visible_tools: Vec<Arc<dyn agent_contracts::ToolSpecView>>,
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
    pub async fn run(
        &self,
        state: &mut RuntimeState,
        mut input: RuntimeInput,
    ) -> Result<RuntimeOutput, agent_types::outcome::AgentError> {
        if input.visible_tools.is_empty() {
            input.visible_tools = self.default_visible_tools.clone();
        }
        if input.runtime_view.is_none() {
            let mut view: Arc<dyn RuntimeView> = match input.interaction.clone() {
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
            for extension in &self.extensions {
                view = extension.wrap_runtime_view(view);
            }
            input.runtime_view = Some(view);
        }
        run_agent_loop(&self.inner, state, input).await
    }

    /// The injected operation backend, if any.
    pub fn operation_backend(&self) -> Option<Arc<dyn OperationBackend>> {
        self.operation_backend.clone()
    }

    /// Return the tool specifications exposed by the configured registry.
    pub fn visible_tools(&self) -> Vec<Arc<dyn agent_contracts::ToolSpecView>> {
        self.default_visible_tools.clone()
    }
}

/// Builder for [`Runtime`].
pub struct RuntimeBuilder {
    inner: AgentRuntimeBuilder,
    operation_backend: Option<Arc<dyn OperationBackend>>,
    extensions: Vec<Arc<dyn RuntimeExtension>>,
}

impl RuntimeBuilder {
    /// Create an empty builder.
    pub fn new() -> Self {
        Self {
            inner: AgentRuntime::builder(),
            operation_backend: None,
            extensions: Vec::new(),
        }
    }

    /// Inject the caller-owned LLM provider.
    pub fn llm_provider(mut self, provider: Arc<LlmProviderWrapper>) -> Self {
        self.inner = self.inner.llm_provider(provider);
        self
    }

    /// Inject the compression pipeline.
    pub fn compression_pipeline(mut self, pipeline: Arc<dyn CompressionPipeline>) -> Self {
        self.inner = self.inner.compression_pipeline(pipeline);
        self
    }

    /// Inject the prompt builder.
    pub fn prompt_builder(mut self, builder: Arc<dyn PromptBuilder>) -> Self {
        self.inner = self.inner.prompt_builder(builder);
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<Arc<str>>) -> Self {
        self.inner = self.inner.system_prompt(prompt);
        self
    }

    /// Inject the tool registry.
    pub fn tool_registry(mut self, registry: Arc<dyn ToolRegistry>) -> Self {
        self.inner = self.inner.tool_registry(registry);
        self
    }

    /// Inject the skill registry.
    pub fn skill_registry(mut self, registry: Arc<dyn SkillRegistry>) -> Self {
        self.inner = self.inner.skill_registry(registry);
        self
    }

    /// Set feature flags.
    pub fn feature_flags(mut self, flags: FeatureFlags) -> Self {
        self.inner = self.inner.feature_flags(flags);
        self
    }

    /// Set the maximum number of turns.
    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.inner = self.inner.max_turns(max_turns);
        self
    }

    /// Set the token budget configuration.
    pub fn token_budget_config(mut self, config: TokenBudgetConfig) -> Self {
        self.inner = self.inner.token_budget_config(config);
        self
    }

    /// Inject the token budget policy.
    pub fn token_budget_policy(mut self, policy: Arc<dyn TokenBudgetPolicy>) -> Self {
        self.inner = self.inner.token_budget_policy(policy);
        self
    }

    /// Inject one caller-owned operation backend instance.
    pub fn operation_backend(mut self, backend: Arc<dyn OperationBackend>) -> Self {
        self.operation_backend = Some(backend);
        self
    }

    /// Install a narrow runtime extension.
    pub fn extension(mut self, extension: Arc<dyn RuntimeExtension>) -> Self {
        self.extensions.push(extension);
        self
    }

    /// Build the runtime.  Missing fields are reported by the core builder.
    pub fn build(self) -> Result<Runtime, BuildError> {
        let default_visible_tools = self.inner.build().map(|inner| {
            let tools = inner
                .tool_registry()
                .list_specs()
                .into_iter()
                .map(|spec| tool::ToolSpecSnapshot::from(spec))
                .map(|spec| Arc::new(spec) as Arc<dyn agent_contracts::ToolSpecView>)
                .collect();
            (inner, tools)
        })?;
        Ok(Runtime {
            inner: default_visible_tools.0,
            operation_backend: self.operation_backend,
            extensions: self.extensions,
            default_visible_tools: default_visible_tools.1,
        })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A no-op extension useful for feature-gated integrations.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopRuntimeExtension;

impl RuntimeExtension for NoopRuntimeExtension {
    fn wrap_runtime_view(&self, view: Arc<dyn RuntimeView>) -> Arc<dyn RuntimeView> {
        view
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_state_is_caller_owned() {
        let state = xiaoo_core::LoopState::new("conversation-1".to_string());
        assert_eq!(state.session_id, "conversation-1");
        assert!(state.messages.read().is_empty());
    }
}
