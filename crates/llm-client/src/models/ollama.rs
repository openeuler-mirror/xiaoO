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

    pub(crate) async fn get_context_length(&self, model: &str) -> Result<Option<u64>, LlmError> {
        let url = format!("{}/api/show", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "Failed to show model details: HTTP {}: {}",
                status, body
            )));
        }

        let body: serde_json::Value = response.json().await.map_err(map_reqwest_error)?;
        Ok(find_context_length_in_value(&body, None))
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

fn find_context_length_in_value(
    value: &serde_json::Value,
    parent_key: Option<&str>,
) -> Option<u64> {
    match value {
        serde_json::Value::Object(map) => map.iter().find_map(|(key, child)| {
            find_context_length_in_value(child, Some(key)).or_else(|| {
                if child.is_number() && key_looks_like_context_length(key) {
                    child.as_u64()
                } else {
                    None
                }
            })
        }),
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|item| find_context_length_in_value(item, parent_key)),
        serde_json::Value::Number(number) => {
            parent_key.filter(|key| key_looks_like_context_length(key))?;
            number.as_u64()
        }
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
    use super::find_context_length_in_value;

    #[test]
    fn finds_ollama_model_info_context_length() {
        let body = serde_json::json!({
            "model_info": {
                "llama.context_length": 32768
            }
        });

        assert_eq!(find_context_length_in_value(&body, None), Some(32768));
    }
}
