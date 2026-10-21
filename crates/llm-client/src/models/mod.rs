use std::time::Duration;

mod anthropic;
mod gemini;
mod ollama;
mod openai_family;
mod openrouter;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;
use crate::provider_registry::ProtocolFamily;
use crate::resolver::ResolvedConfig;

pub(crate) use anthropic::AnthropicModelCatalog;
pub(crate) use gemini::GeminiModelCatalog;
pub(crate) use ollama::OllamaModelCatalog;
pub(crate) use openai_family::OpenAiFamilyModelCatalog;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

impl ModelSummary {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: None,
            provider: None,
            context_length: None,
            max_output_tokens: None,
            raw: None,
        }
    }
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }
    pub fn with_context_length(mut self, length: u64) -> Self {
        self.context_length = Some(length);
        self
    }
    pub fn with_max_output_tokens(mut self, tokens: u64) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }
    pub fn with_raw(mut self, raw: serde_json::Value) -> Self {
        self.raw = Some(raw);
        self
    }
}

#[async_trait]
pub trait ModelCatalog: Send + Sync {
    async fn list_models(&self) -> Result<Vec<ModelSummary>, LlmError>;
}

/// 解析模型上下文窗口，优先级：provider 动态 catalog > OpenRouter 公共模型库
/// > Ollama `/api/show`。
///
/// 全链 fail-soft：任一级失败只记 warn 并落穿到下一级，绝不因网络问题
/// 阻塞 provider 装配（调用方在全部未命中时使用 provider 构造 fallback）。
pub async fn resolve_model_context_length(
    config: &ResolvedConfig,
    model: &str,
) -> Result<Option<u64>, LlmError> {
    if config.supports_model_catalog {
        let catalog_result = match create_model_catalog(config) {
            Ok(catalog) => catalog.list_models().await,
            Err(error) => {
                tracing::warn!(
                    model = %model,
                    error = %error,
                    "provider model catalog unavailable; continuing resolution chain"
                );
                Ok(Vec::new())
            }
        };
        match catalog_result {
            Ok(models) => {
                if let Some(context) = find_model_summary(&models, model).and_then(|summary| {
                    summary.context_length.or_else(|| {
                        summary
                            .raw
                            .as_ref()
                            .and_then(extract_context_length_from_raw)
                    })
                }) {
                    return Ok(Some(context));
                }
            }
            Err(error) => {
                tracing::warn!(
                    model = %model,
                    error = %error,
                    "provider model catalog request failed; continuing resolution chain"
                );
            }
        }
    }

    if let Some(context) = openrouter::lookup_context_length(model).await {
        return Ok(Some(context));
    }

    match config.protocol {
        ProtocolFamily::Ollama => {
            OllamaModelCatalog::new(config.base_url.clone())
                .get_context_length(model)
                .await
        }
        _ => Ok(None),
    }
}

pub fn create_model_catalog(config: &ResolvedConfig) -> Result<Box<dyn ModelCatalog>, LlmError> {
    match config.protocol {
        ProtocolFamily::OpenAiCompatible => {
            let api_key = config.api_key.clone().unwrap_or_default();
            Ok(Box::new(OpenAiFamilyModelCatalog::new(
                api_key,
                config.base_url.clone(),
            )))
        }
        ProtocolFamily::Anthropic => {
            let api_key = config.api_key.clone().ok_or_else(|| {
                LlmError::ConfigError("API key required for model catalog".to_string())
            })?;
            Ok(Box::new(AnthropicModelCatalog::new(
                api_key,
                config.base_url.clone(),
            )))
        }
        ProtocolFamily::Gemini => {
            let api_key = config.api_key.clone().ok_or_else(|| {
                LlmError::ConfigError("API key required for model catalog".to_string())
            })?;
            Ok(Box::new(GeminiModelCatalog::new(
                api_key,
                config.base_url.clone(),
            )))
        }
        ProtocolFamily::Ollama => Ok(Box::new(OllamaModelCatalog::new(config.base_url.clone()))),
        ProtocolFamily::Zhipu => {
            let api_key = config.api_key.clone().ok_or_else(|| {
                LlmError::ConfigError("API key required for model catalog".to_string())
            })?;
            Ok(Box::new(OpenAiFamilyModelCatalog::new(
                api_key,
                config.base_url.clone(),
            )))
        }
    }
}

fn find_model_summary<'a>(models: &'a [ModelSummary], model: &str) -> Option<&'a ModelSummary> {
    let target = normalize_model_id(model);
    models
        .iter()
        .find(|summary| normalize_model_id(&summary.id) == target)
}

fn normalize_model_id(model: &str) -> String {
    model
        .trim()
        .strip_prefix("models/")
        .unwrap_or(model.trim())
        .to_ascii_lowercase()
}

fn extract_context_length_from_raw(raw: &serde_json::Value) -> Option<u64> {
    match raw {
        serde_json::Value::Object(map) => map.iter().find_map(|(key, value)| {
            if value.is_number() && key_looks_like_context_length(key) {
                value.as_u64()
            } else {
                extract_context_length_from_raw(value)
            }
        }),
        serde_json::Value::Array(items) => items.iter().find_map(extract_context_length_from_raw),
        _ => None,
    }
}

fn key_looks_like_context_length(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    normalized == "maxinputtokens"
        || normalized == "inputtokenlimit"
        || normalized == "contextwindow"
        || normalized == "maxcontextwindow"
        || normalized.ends_with("contextlength")
}

/// 模型目录客户端统一的 HTTP client。
///
/// 这些客户端此前是裸 `reqwest::Client::builder()`（严格校验），于是自签名网关下
/// chat 能用、目录拉取却在握手阶段失败——表现为"列表一直是内置列表"。
/// 是否放宽校验由 [`crate::http::tls_verification_disabled`] 决定，与 chat provider 同源。
pub(crate) fn model_catalog_http_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(crate::http::tls_verification_disabled())
        .build()
        .unwrap_or_else(crate::http::client_build_failed)
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/models_test.rs"]
mod tests;
