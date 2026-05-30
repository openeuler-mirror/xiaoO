use std::sync::Arc;

pub type ApiKeyProviderFn = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Clone)]
pub struct LlmProviderConfig {
    pub provider: String,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub model: String,
    pub api_key_env: Option<String>,
    pub api_key_provider: Option<ApiKeyProviderFn>,
}

impl LlmProviderConfig {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            api_key: None,
            api_base: None,
            model: model.into(),
            api_key_env: None,
            api_key_provider: None,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = Some(api_base.into());
        self
    }

    pub fn with_api_key_provider(mut self, provider: ApiKeyProviderFn) -> Self {
        self.api_key_provider = Some(provider);
        self
    }
}
