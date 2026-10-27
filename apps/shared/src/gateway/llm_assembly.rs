//! LLM provider assembly facade.
//!
//! [`build_llm_provider`] is the single entry point for the provider
//! assembly chain (resolve config + api-key fallback + build provider +
//! context window); the shared resolver (`hosted_runtime_resolver`) and
//! serverside both route through it so the two assembly paths cannot drift.
//!
//! Two hard constraints:
//! - No fine-grained llm-client types in the API surface:
//!   `ResolveInput` / `ResolvedConfig` / `create_llm_provider_from_resolved`
//!   / `resolve_provider_profile` / `resolve_model_context_length` stay
//!   internal to this module.
//! - Every pub item must have an app consumer (currently the serverside
//!   `daemon_runtime` startup assembly and the resolver cache-miss path);
//!   `llm-client` never leaks out of this module.
//!
//! The return type is the existing `xiaoo_api::llm` export
//! (`LlmProviderWrapper`).

use std::env;
use std::sync::Arc;
use std::time::Duration;

use llm_client::{
    create_llm_provider_from_resolved, create_model_catalog, resolve_provider_profile, ChatMessage,
    ContentBlock, LlmProviderWrapper, LlmRequest, MessageRole, ReasoningEffort, ToolChoice,
};
use serde::Serialize;
use xiaoo_api::chat::AgentId;
use xiaoo_api::llm::{resolve_config, resolve_model_context_length, ResolveInput};

/// LLM provider assembly input. Fields are plain types plus already-exported
/// shared handles.
#[derive(Clone, Debug)]
pub struct LlmAssemblyInput {
    pub provider: String,
    pub model: String,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    /// Explicit context-window override; `None` or `0` falls back to dynamic
    /// probing + static capabilities.
    pub context_window_override: Option<u32>,
    pub agent_id: Option<AgentId>,
}

/// Errors from the LLM provider assembly chain.
#[derive(Debug, thiserror::Error)]
pub enum LlmAssemblyError {
    #[error("failed to resolve llm provider config: {0}")]
    Resolve(String),
    #[error("failed to create llm provider: {0}")]
    Create(String),
    #[error("missing required API key environment variable: {0}")]
    MissingApiKeyEnv(String),
    #[error("API key environment variable is not valid unicode: {0}")]
    InvalidApiKeyEnv(String),
    #[error("provider connection test timed out")]
    ProbeTimeout,
    #[error("provider connection test failed: {0}")]
    Probe(String),
    #[error("provider `{0}` does not expose a model catalog")]
    ModelCatalogUnsupported(String),
    #[error("failed to list provider models: {0}")]
    ModelCatalog(String),
}

/// Secret-free model metadata returned to application management surfaces.
#[derive(Clone, Debug, Serialize)]
pub struct LlmModelSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

/// Resolve the api-key: explicit > environment variable > provider profile
/// default env.
///
/// - `api_key` set → use it.
/// - `api_key_env` set → look it up in the process environment; a missing
///   variable is an error (app assembly treats api_key_env as a hard
///   requirement).
/// - Neither set → fall back to the provider profile's
///   `default_api_key_env` (if any); a missing variable is tolerated there
///   (the profile may allow anonymous access).
fn resolve_api_key(input: &LlmAssemblyInput) -> Result<Option<String>, LlmAssemblyError> {
    if let Some(api_key) = input.api_key.as_ref() {
        return Ok(Some(api_key.clone()));
    }

    if let Some(env_name) = input.api_key_env.as_deref() {
        return resolve_api_key_env(env_name, true);
    }

    if let Some(env_name) = resolve_provider_profile(&input.provider)
        .and_then(|profile| profile.default_api_key_env.map(str::to_string))
    {
        return resolve_api_key_env(&env_name, false);
    }

    Ok(None)
}

fn resolve_api_key_env(
    env_name: &str,
    fail_when_missing: bool,
) -> Result<Option<String>, LlmAssemblyError> {
    match env::var(env_name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(env::VarError::NotPresent) if fail_when_missing => {
            Err(LlmAssemblyError::MissingApiKeyEnv(env_name.to_string()))
        }
        Ok(_) | Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(LlmAssemblyError::InvalidApiKeyEnv(env_name.to_string()))
        }
    }
}

/// Resolve config → build provider → pin the context window, in one step.
///
/// Returns `(provider, effective_context_window)`: window precedence is
/// explicit override > dynamic probing > provider capabilities static
/// fallback (≥ 1), matching [`resolve_model_context_length`].
pub async fn build_llm_provider(
    input: LlmAssemblyInput,
) -> Result<(Arc<LlmProviderWrapper>, u32), LlmAssemblyError> {
    let api_key = resolve_api_key(&input)?;

    let resolved = resolve_config(ResolveInput {
        provider: Some(input.provider.clone()),
        protocol: None,
        api_key,
        api_key_env: None,
        base_url: input.api_base.clone(),
    })
    .map_err(|e| LlmAssemblyError::Resolve(e.to_string()))?;

    let agent_id = input.agent_id.as_ref().map(|id| id.0.clone());
    let created = create_llm_provider_from_resolved(&resolved, input.model.clone(), agent_id, None)
        .map_err(|e| LlmAssemblyError::Create(e.to_string()))?;

    let provider = Arc::new(created);
    let static_fallback = provider.capabilities().max_context_window;

    let effective_context_window = resolve_effective_context_window(
        &resolved,
        &input.model,
        input.context_window_override,
        static_fallback,
    )
    .await;

    // Saturate rather than truncate on the usize → u32 conversion: real
    // model windows sit far below u32::MAX, and a truncated value would
    // silently mislead callers.
    let context_window = u32::try_from(effective_context_window).unwrap_or(u32::MAX);

    Ok((provider, context_window))
}

/// Test the configured provider, credentials and model with a minimal request.
///
/// This intentionally uses the same resolver and secret lookup path as runtime
/// assembly. The response body is discarded and credentials never leave this
/// facade.
pub async fn probe_llm_provider(input: LlmAssemblyInput) -> Result<(), LlmAssemblyError> {
    let api_key = resolve_api_key(&input)?;
    let resolved = resolve_config(ResolveInput {
        provider: Some(input.provider),
        protocol: None,
        api_key,
        api_key_env: None,
        base_url: input.api_base,
    })
    .map_err(|error| LlmAssemblyError::Resolve(error.to_string()))?;
    let provider = create_llm_provider_from_resolved(&resolved, input.model, None, None)
        .map_err(|error| LlmAssemblyError::Create(error.to_string()))?;
    let request = LlmRequest {
        messages: vec![ChatMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "Reply with OK.".to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        }],
        tools: vec![],
        tool_choice: ToolChoice::None,
        max_tokens: Some(8),
        temperature: Some(0.0),
        response_format: Default::default(),
        reasoning_effort: ReasoningEffort::Off,
    };
    tokio::time::timeout(Duration::from_secs(20), provider.complete(&request))
        .await
        .map_err(|_| LlmAssemblyError::ProbeTimeout)?
        .map_err(|error| LlmAssemblyError::Probe(error.to_string()))?;
    Ok(())
}

/// List models using the provider's native catalog endpoint.
///
/// Raw provider responses are deliberately omitted from the public result so
/// management clients receive a stable, bounded contract.
pub async fn list_llm_models(
    input: LlmAssemblyInput,
) -> Result<Vec<LlmModelSummary>, LlmAssemblyError> {
    let api_key = resolve_api_key(&input)?;
    let requested_provider = input.provider.clone();
    let resolved = resolve_config(ResolveInput {
        provider: Some(input.provider),
        protocol: None,
        api_key,
        api_key_env: None,
        base_url: input.api_base,
    })
    .map_err(|error| LlmAssemblyError::Resolve(error.to_string()))?;
    if !resolved.supports_model_catalog {
        return Err(LlmAssemblyError::ModelCatalogUnsupported(
            resolved.provider.unwrap_or(requested_provider),
        ));
    }
    let catalog = create_model_catalog(&resolved)
        .map_err(|error| LlmAssemblyError::ModelCatalog(error.to_string()))?;
    let models = tokio::time::timeout(Duration::from_secs(20), catalog.list_models())
        .await
        .map_err(|_| LlmAssemblyError::ProbeTimeout)?
        .map_err(|error| LlmAssemblyError::ModelCatalog(error.to_string()))?;
    Ok(models
        .into_iter()
        .map(|model| LlmModelSummary {
            id: model.id,
            display_name: model.display_name,
            provider: model.provider,
            context_length: model.context_length,
            max_output_tokens: model.max_output_tokens,
        })
        .collect())
}

/// Context-window resolution used by [`build_llm_provider`]: explicit
/// override > dynamic probing > capabilities fallback.
async fn resolve_effective_context_window(
    resolved: &llm_client::ResolvedConfig,
    model: &str,
    configured: Option<u32>,
    static_fallback: usize,
) -> usize {
    if let Some(context_window) = configured.filter(|value| *value > 0) {
        return context_window as usize;
    }

    match resolve_model_context_length(resolved, model).await {
        Ok(Some(context_window)) => match usize::try_from(context_window) {
            Ok(value) if value > 0 => return value,
            Ok(_) => {}
            Err(_) => {}
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
