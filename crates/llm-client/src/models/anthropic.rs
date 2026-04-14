use crate::error::{map_reqwest_error, LlmError};
use crate::models::{ModelCatalog, ModelSummary};
use async_trait::async_trait;
use std::time::Duration;

pub(crate) struct AnthropicModelCatalog {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
}

impl AnthropicModelCatalog {
    pub(crate) fn new(api_key: String, api_base: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_key,
            api_base,
        }
    }
}

#[async_trait]
impl ModelCatalog for AnthropicModelCatalog {
    async fn list_models(&self) -> Result<Vec<ModelSummary>, LlmError> {
        let url = format!("{}/models", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .get(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
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
        let models = body["data"]
            .as_array()
            .ok_or_else(|| LlmError::ParseError("Invalid models response format".to_string()))?;
        Ok(models
            .iter()
            .filter_map(|model| {
                model["id"].as_str().map(|id| {
                    let mut summary = ModelSummary::new(id)
                        .with_provider("anthropic")
                        .with_raw(model.clone());
                    if let Some(display_name) = model["display_name"].as_str() {
                        summary = summary.with_display_name(display_name);
                    }
                    summary
                })
            })
            .collect())
    }
}
