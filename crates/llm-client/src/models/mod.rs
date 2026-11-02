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
use openrouter::OpenRouterLookup;

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

/// 探测链各阶段的等待预算（按目录 > OpenRouter > Ollama 的解析顺序）。
///
/// 各 catalog 客户端自身的 HTTP 上限（30s/10s）保持不变——模型列表浏览
/// 等用户交互路径仍依赖它们；探测链内由这里的阶段预算截断等待。
const CATALOG_STAGE_BUDGET: Duration = Duration::from_secs(2);
const OPENROUTER_STAGE_BUDGET: Duration = Duration::from_secs(3);
const OLLAMA_STAGE_BUDGET: Duration = Duration::from_secs(2);

/// [`resolve_model_context_length`] 的结论三态。
///
/// 关键区分在两种"未命中"之间：只有每个可用数据源都被正常咨询（请求
/// 成功、数据新鲜）后的未命中才是事实，调用方可以长期缓存；任何一级
/// 失败、超时、被禁用或数据过期都只能算 [`Self::Degraded`]——把降级
/// 结论当事实缓存，会在网络恢复后继续冻结重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelContextResolution {
    /// 命中：某级数据源给出了该模型的上下文窗口。
    Found(u64),
    /// 确证未命中：链上可用源全部被正常咨询且均无该模型。
    NotFound,
    /// 降级：至少一级未被正常咨询（失败/超时/禁用/数据过期）。
    Degraded,
}

/// 解析模型上下文窗口，优先级：provider 动态 catalog > OpenRouter 公共模型库
/// > Ollama `/api/show`。
///
/// 全链 fail-soft：任一级失败（含阶段预算超时）只记 warn 并落穿到下一级，
/// 绝不因网络问题阻塞 provider 装配（调用方在未命中时使用 provider 构造
/// fallback）。返回值同时给未命中分级：[`ModelContextResolution::NotFound`]
/// 可作为长期负结论缓存；[`ModelContextResolution::Degraded`] 的未命中
/// 不可信，调用方不得据此下长期结论。
pub async fn resolve_model_context_length(
    config: &ResolvedConfig,
    model: &str,
) -> ModelContextResolution {
    let mut degraded = false;

    if config.supports_model_catalog {
        match create_model_catalog(config) {
            Ok(catalog) => {
                match tokio::time::timeout(CATALOG_STAGE_BUDGET, catalog.list_models()).await {
                    Ok(Ok(models)) => {
                        if let Some(context) =
                            find_model_summary(&models, model).and_then(|summary| {
                                summary.context_length.or_else(|| {
                                    summary
                                        .raw
                                        .as_ref()
                                        .and_then(extract_context_length_from_raw)
                                })
                            })
                        {
                            return ModelContextResolution::Found(context);
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(
                            model = %model,
                            error = %error,
                            "provider model catalog request failed; continuing resolution chain"
                        );
                        degraded = true;
                    }
                    Err(_) => {
                        tracing::warn!(
                            model = %model,
                            "provider model catalog timed out after {}ms; continuing resolution chain",
                            CATALOG_STAGE_BUDGET.as_millis()
                        );
                        degraded = true;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(
                    model = %model,
                    error = %error,
                    "provider model catalog unavailable; continuing resolution chain"
                );
                degraded = true;
            }
        }
    }

    match tokio::time::timeout(
        OPENROUTER_STAGE_BUDGET,
        openrouter::lookup_context_length(model),
    )
    .await
    {
        Ok(OpenRouterLookup::Found(context)) => {
            return ModelContextResolution::Found(context);
        }
        Ok(OpenRouterLookup::Miss) => {}
        // 拉取失败在 openrouter 层已 warn，这里保持静默的是禁用查询与
        // 过期数据上的未命中——两者共同点是无法证明"公共库没有该模型"。
        Ok(OpenRouterLookup::Unavailable) => {
            degraded = true;
        }
        Err(_) => {
            tracing::warn!(
                model = %model,
                "openrouter model lookup timed out; continuing resolution chain"
            );
            degraded = true;
        }
    }

    if let ProtocolFamily::Ollama = config.protocol {
        match tokio::time::timeout(
            OLLAMA_STAGE_BUDGET,
            OllamaModelCatalog::new(config.base_url.clone()).get_context_length(model),
        )
        .await
        {
            Ok(Ok(Some(context))) => return ModelContextResolution::Found(context),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    model = %model,
                    error = %error,
                    "ollama /api/show probe failed; giving up resolution chain"
                );
                degraded = true;
            }
            Err(_) => {
                tracing::warn!(
                    model = %model,
                    "ollama /api/show probe timed out; giving up resolution chain"
                );
                degraded = true;
            }
        }
    }

    if degraded {
        ModelContextResolution::Degraded
    } else {
        ModelContextResolution::NotFound
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
/// 是否放宽 TLS 校验由 [`crate::http::tls_verification_disabled`] 决定，与 chat
/// provider 同源：若这里各自用严格的 `reqwest::Client::builder()`，自签名网关下
/// 会出现 chat 能用、目录拉取却在握手阶段失败——表现为"列表一直是内置列表"。
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
