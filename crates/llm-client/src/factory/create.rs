use std::env;
use std::sync::Arc;

use agent_contracts::runtime::RuntimeView;
use agent_contracts::LlmProvider;

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
) -> Result<LlmProviderWrapper, LlmError> {
    // 注册表键先行克隆：各协议分支会消费原始 api_key，限流器则按
    // (api_base, api_key) 归一后共享（见下方 gate_provider 调用）。
    let registry_api_key = api_key.clone();
    let raw: Arc<dyn LlmProvider> = match protocol {
        ProtocolFamily::OpenAiCompatible => {
            let api_key = if profile.api_key_required {
                api_key.ok_or_else(|| {
                    LlmError::ConfigError(format!("{} API key required", profile.display_name))
                })?
            } else {
                api_key.unwrap_or_default()
            };
            // reasoning_effort 方言来自 provider 注册表声明（zai-coding-plan
            // 等端点为 Zhipu API 的 OpenAI 兼容 provider 在此声明 Zhipu 方言），
            // 新增 provider 无需改工厂代码。
            Arc::new(
                OpenAiFamilyProvider::new(
                    api_key,
                    api_base.clone(),
                    model,
                    OpenAiFamilyAuthStyle::Bearer,
                    vec![],
                )
                .with_reasoning_dialect(profile.reasoning_dialect),
            )
        }
        ProtocolFamily::Anthropic => {
            let api_key = api_key.ok_or_else(|| {
                LlmError::ConfigError(format!("{} API key required", profile.display_name))
            })?;
            Arc::new(AnthropicProvider::new(api_key, api_base.clone(), model))
        }
        ProtocolFamily::Gemini => {
            let api_key = api_key.ok_or_else(|| {
                LlmError::ConfigError(format!("{} API key required", profile.display_name))
            })?;
            Arc::new(GeminiProvider::new(api_key, api_base.clone(), model))
        }
        ProtocolFamily::Ollama => Arc::new(OllamaProvider::new(api_base.clone(), model)),
        ProtocolFamily::Zhipu => {
            let api_key = api_key.ok_or_else(|| {
                LlmError::ConfigError(format!("{} API key required", profile.display_name))
            })?;
            Arc::new(ZhipuProvider::new(api_key, api_base.clone(), model))
        }
    };
    // 准入闸门套在裸 provider 上、wrapper 之内：位于重试循环之下，每次
    // 重试尝试都经过闸门计数（重试同样消耗网关配额）。Ollama 为本地
    // 运行时，无外部配额，仅在显式配置后受闸门约束。
    let gated = crate::rate_limit::gate_provider(
        raw,
        &api_base,
        registry_api_key,
        protocol == ProtocolFamily::Ollama,
    );
    Ok(LlmProviderWrapper::new(gated, agent_id, runtime_view))
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
    Ok(normalize_api_base(api_base))
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
        supports_model_catalog: false,
        // 无需按协议特判方言：Zhipu 协议族由 ZhipuProvider 原生内置 Zhipu
        // 方言（不读此字段），其余协议均为默认 OpenAi 方言，由
        // `..default()` 补齐。
        ..ProviderProfile::default()
    }
}
