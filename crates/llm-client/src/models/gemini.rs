use crate::error::{map_reqwest_error, LlmError};
use crate::models::{ModelCatalog, ModelSummary};
use async_trait::async_trait;
use std::time::Duration;

pub(crate) struct GeminiModelCatalog {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
}

impl GeminiModelCatalog {
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
impl ModelCatalog for GeminiModelCatalog {
    async fn list_models(&self) -> Result<Vec<ModelSummary>, LlmError> {
        let url = format!(
            "{}/v1beta/models?key={}",
            self.api_base.trim_end_matches('/'),
            self.api_key
        );
        let response = self
            .client
            .get(&url)
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
        let models = body["models"]
            .as_array()
            .ok_or_else(|| LlmError::ParseError("Invalid models response format".to_string()))?;
        Ok(models
            .iter()
            .filter_map(|model| {
                let id = model["name"]
                    .as_str()
                    .and_then(|n| n.strip_prefix("models/"))
                    .or_else(|| model["name"].as_str());
                id.map(|id| {
                    let mut summary = ModelSummary::new(id)
                        .with_provider("google")
                        .with_raw(model.clone());
                    if let Some(display_name) = model["displayName"].as_str() {
                        summary = summary.with_display_name(display_name);
                    }
                    if let Some(limit) = model["inputTokenLimit"].as_u64() {
                        summary = summary.with_context_length(limit);
                    }
                    if let Some(limit) = model["outputTokenLimit"].as_u64() {
                        summary = summary.with_max_output_tokens(limit);
                    }
                    summary
                })
            })
            .collect())
    }
}
