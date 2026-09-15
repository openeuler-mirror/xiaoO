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
    let err = match Runtime::builder().llm_provider(Arc::new(wrapper)).build() {
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
