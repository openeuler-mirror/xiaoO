use crate::error::{map_reqwest_error, LlmError};
use crate::models::{ModelCatalog, ModelSummary};
use async_trait::async_trait;
use std::time::Duration;

pub(crate) struct OllamaModelCatalog {
    client: reqwest::Client,
    api_base: String,
}

impl OllamaModelCatalog {
    pub(crate) fn new(api_base: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_base,
        }
    }
}

#[async_trait]
impl ModelCatalog for OllamaModelCatalog {
    async fn list_models(&self) -> Result<Vec<ModelSummary>, LlmError> {
        let url = format!("{}/api/tags", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "Failed to list models: HTTP {}: {}",
                status, body
            )));
        }
        let body: serde_json::Value = response.json().await.map_err(map_reqwest_error)?;
        let models = body["models"]
            .as_array()
            .ok_or_else(|| LlmError::ParseError("Invalid models response format".to_string()))?;
        Ok(models
            .iter()
            .filter_map(|model| {
                model["name"].as_str().map(|name| {
                    let mut summary = ModelSummary::new(name)
                        .with_provider("ollama")
                        .with_display_name(name)
                        .with_raw(model.clone());
                    if let Some(family) = model["details"]["family"].as_str() {
                        summary = summary.with_provider(family);
                    }
                    summary
                })
            })
            .collect())
    }
}
