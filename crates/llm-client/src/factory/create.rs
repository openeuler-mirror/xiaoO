use std::env;
use std::sync::Arc;

use agent_contracts::runtime::RuntimeView;

use crate::error::LlmError;
use crate::provider_registry::{
    normalize_api_base, resolve_provider_profile, ProtocolFamily, ProviderProfile,
};
use crate::providers::{
    AnthropicProvider, GeminiProvider, OllamaProvider, OpenAiFamilyAuthStyle, OpenAiFamilyProvider,
    ZhipuProvider,
};
use crate::resolver::ResolvedConfig;

use super::config::LlmProviderConfig;
use super::wrapper::LlmProviderWrapper;

pub fn create_llm_provider(
    config: &LlmProviderConfig,
    agent_id: Option<String>,
    runtime_view: Option<Arc<dyn RuntimeView>>,
) -> Result<LlmProviderWrapper, LlmError> {
    let profile = resolve_provider_profile(&config.provider).ok_or_else(|| {
        LlmError::ProviderNotFound(format!(
            "Unknown provider: {}. Supported: {}",
            config.provider,
            crate::provider_registry::supported_providers().join(", ")
        ))
    })?;

    let api_base = resolve_api_base(config, &profile)?;
    let api_key = resolve_api_key(config, &profile)?;

    create_provider_by_protocol(
        profile.protocol_family,
        api_key,
        api_base,
        config.model.clone(),
        &profile,
        agent_id,
        runtime_view,
        config.api_key_provider.clone(),
    )
}

pub fn create_llm_provider_from_resolved(
    config: &ResolvedConfig,
    model: String,
    agent_id: Option<String>,
    runtime_view: Option<Arc<dyn RuntimeView>>,
) -> Result<LlmProviderWrapper, LlmError> {
    let profile = config
        .provider
        .as_deref()
        .and_then(resolve_provider_profile);

    create_provider_by_protocol(
        config.protocol,
        config.api_key.clone(),
        config.base_url.clone(),
        model,
        &profile.unwrap_or_else(|| default_profile(config.protocol)),
        agent_id,
        runtime_view,
        None,
    )
}

fn create_provider_by_protocol(
    protocol: ProtocolFamily,
    api_key: Option<String>,
    api_base: String,
    model: String,
    profile: &ProviderProfile,
    agent_id: Option<String>,
    runtime_view: Option<Arc<dyn RuntimeView>>,
    api_key_provider: Option<crate::factory::ApiKeyProviderFn>,
) -> Result<LlmProviderWrapper, LlmError> {
    match protocol {
        ProtocolFamily::OpenAiCompatible => {
            if profile.api_key_required && api_key_provider.is_none() && api_key.is_none() {
                return Err(LlmError::ConfigError(format!("{} API key required", profile.display_name)));
            }
            Ok(LlmProviderWrapper::new(
                Arc::new(OpenAiFamilyProvider::new(
                    api_key,
                    api_base,
                    model,
                    OpenAiFamilyAuthStyle::Bearer,
                    vec![],
                    api_key_provider,
                )),
                agent_id,
                runtime_view,
            ))
        }
        ProtocolFamily::Anthropic => {
            if profile.api_key_required && api_key_provider.is_none() && api_key.is_none() {
                return Err(LlmError::ConfigError(format!("{} API key required", profile.display_name)));
            }
            Ok(LlmProviderWrapper::new(
                Arc::new(AnthropicProvider::new(api_key, api_base, model, api_key_provider)),
                agent_id,
                runtime_view,
            ))
        }
        ProtocolFamily::Gemini => {
            if profile.api_key_required && api_key_provider.is_none() && api_key.is_none() {
                return Err(LlmError::ConfigError(format!("{} API key required", profile.display_name)));
            }
            Ok(LlmProviderWrapper::new(
                Arc::new(GeminiProvider::new(api_key, api_base, model, api_key_provider)),
                agent_id,
                runtime_view,
            ))
        }
        ProtocolFamily::Ollama => Ok(LlmProviderWrapper::new(
            Arc::new(OllamaProvider::new(api_base, model)),
            agent_id,
            runtime_view,
        )),
        ProtocolFamily::Zhipu => {
            if profile.api_key_required && api_key_provider.is_none() && api_key.is_none() {
                return Err(LlmError::ConfigError(format!("{} API key required", profile.display_name)));
            }
            Ok(LlmProviderWrapper::new(
                Arc::new(ZhipuProvider::new(api_key, api_base, model, api_key_provider)),
                agent_id,
                runtime_view,
            ))
        }
    }
}

fn resolve_api_base(
    config: &LlmProviderConfig,
    profile: &ProviderProfile,
) -> Result<String, LlmError> {
    let api_base = match &config.api_base {
        Some(api_base) => api_base.clone(),
        None => profile
            .default_base_url
            .ok_or_else(|| {
                LlmError::ConfigError(format!("{} API base required", profile.display_name))
            })?
            .to_string(),
    };
    let api_base = api_base.trim();
    if api_base.is_empty() {
        return Err(LlmError::ConfigError(format!(
            "{} API base required",
            profile.display_name
        )));
    }
    Ok(normalize_api_base(api_base, profile.api_base_style))
}

fn resolve_api_key(
    config: &LlmProviderConfig,
    profile: &ProviderProfile,
) -> Result<Option<String>, LlmError> {
    if let Some(ref api_key) = config.api_key {
        return Ok(Some(api_key.clone()));
    }
    if let Some(env_name) = profile.default_api_key_env {
        match env::var(env_name) {
            Ok(value) if !value.trim().is_empty() => return Ok(Some(value)),
            Ok(_) | Err(env::VarError::NotPresent) => {}
            Err(env::VarError::NotUnicode(_)) => {
                return Err(LlmError::ConfigError(format!(
                    "environment variable is not valid unicode: {env_name}"
                )));
            }
        }
    }
    match profile.api_key_required {
        true => Err(LlmError::ConfigError(format!(
            "{} API key required",
            profile.display_name
        ))),
        false => Ok(None),
    }
}

fn default_profile(protocol: ProtocolFamily) -> ProviderProfile {
    ProviderProfile {
        provider_name: "custom",
        display_name: "Custom",
        protocol_family: protocol,
        default_base_url: None,
        default_api_key_env: None,
        api_key_required: true,
        api_base_style: crate::provider_registry::ApiBaseStyle::Preserve,
        supports_model_catalog: false,
    }
}
