use crate::error::{map_reqwest_error_raw as map_reqwest_error, LlmError};
use crate::models::{ModelCatalog, ModelSummary};
use async_trait::async_trait;
use std::time::Duration;

pub(crate) struct OpenAiFamilyModelCatalog {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
}

impl OpenAiFamilyModelCatalog {
    pub(crate) fn new(api_key: String, api_base: String) -> Self {
        Self {
            client: super::model_catalog_http_client(Duration::from_secs(30)),
            api_key,
            api_base,
        }
    }
    fn models_url(&self) -> String {
        format!("{}/models", self.api_base.trim_end_matches('/'))
    }
}

#[async_trait]
impl ModelCatalog for OpenAiFamilyModelCatalog {
    async fn list_models(&self) -> Result<Vec<ModelSummary>, LlmError> {
        let mut redactor = crate::error::ErrorRedactor::new(&self.models_url(), &self.api_key);
        let result = async {
            let mut req = self
                .client
                .get(self.models_url())
                .header("Content-Type", "application/json");
            if !self.api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", self.api_key));
            }
            let request = req.build().map_err(map_reqwest_error)?;
            redactor.headers(request.headers());
            let response = self
                .client
                .execute(request)
                .await
                .map_err(map_reqwest_error)?;
            redactor.headers(response.headers());
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(LlmError::ApiError(format!(
                    "Failed to list models: HTTP {}: {}",
                    status, body
                )));
            }
            let body: serde_json::Value = response.json().await.map_err(map_reqwest_error)?;
            let models = body["data"].as_array().ok_or_else(|| {
                LlmError::ParseError("Invalid models response format".to_string())
            })?;
            Ok(models
                .iter()
                .filter_map(|model| {
                    model["id"].as_str().map(|id| {
                        ModelSummary::new(id)
                            .with_display_name(id)
                            .with_raw(model.clone())
                    })
                })
                .collect())
        }
        .await;
        redactor.error_result(result)
    }
}
