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

pub fn create_model_catalog(config: &ResolvedConfig) -> Result<Box<dyn ModelCatalog>, LlmError> {
    match config.protocol {
        ProtocolFamily::OpenAiCompatible => {
            let api_key = config.api_key.clone().ok_or_else(|| {
                LlmError::ConfigError("API key required for model catalog".to_string())
            })?;
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
