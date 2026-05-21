mod anthropic;
mod gemini;
mod ollama;
mod openai_family;

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

pub async fn resolve_model_context_length(
    config: &ResolvedConfig,
    model: &str,
) -> Result<Option<u64>, LlmError> {
    let dynamic_from_catalog = if config.supports_model_catalog {
        let catalog = create_model_catalog(config)?;
        let models = catalog.list_models().await?;
        find_model_summary(&models, model).and_then(|summary| {
            summary.context_length.or_else(|| {
                summary
                    .raw
                    .as_ref()
                    .and_then(extract_context_length_from_raw)
            })
        })
    } else {
        None
    };

    if dynamic_from_catalog.is_some() {
        return Ok(dynamic_from_catalog);
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

#[cfg(test)]
mod tests {
    use super::{extract_context_length_from_raw, find_model_summary, ModelSummary};

    #[test]
    fn find_model_summary_matches_gemini_prefixed_names() {
        let models = vec![ModelSummary::new("gemini-2.5-pro")];
        let found = find_model_summary(&models, "models/gemini-2.5-pro");
        assert!(found.is_some());
    }

    #[test]
    fn extract_context_length_from_raw_finds_max_input_tokens() {
        let raw = serde_json::json!({
            "id": "claude-sonnet",
            "max_input_tokens": 200000
        });

        assert_eq!(extract_context_length_from_raw(&raw), Some(200000));
    }
}
