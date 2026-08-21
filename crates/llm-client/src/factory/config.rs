#[derive(Debug, Clone)]
pub struct LlmProviderConfig {
    pub provider: String,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub model: String,
}

impl LlmProviderConfig {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            api_key: None,
            api_base: None,
            model: model.into(),
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
}
