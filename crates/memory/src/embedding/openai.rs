use async_trait::async_trait;

use crate::{MemoryError, MemoryResult};

use super::EmbeddingProvider;

/// OpenAI-compatible embedding provider.
///
/// Works with OpenAI, OpenRouter, and any custom endpoint that exposes
/// `POST /v1/embeddings` with `{ "model": "...", "input": [...] }`.
pub struct OpenAiEmbedding {
    base_url: String,
    api_key: String,
    model: String,
    dims: usize,
    client: reqwest::Client,
}

impl OpenAiEmbedding {
    pub fn new(base_url: &str, api_key: &str, model: &str, dims: usize) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            dims,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn embeddings_url(&self) -> String {
        let base = &self.base_url;
        if base.ends_with("/embeddings") {
            return base.clone();
        }
        if base.ends_with("/v1") {
            return format!("{base}/embeddings");
        }
        format!("{base}/v1/embeddings")
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedding {
    fn name(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[&str]) -> MemoryResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let response = self
            .client
            .post(self.embeddings_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| MemoryError::Embedding {
                message: format!("embedding HTTP request failed: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(MemoryError::Embedding {
                message: format!("embedding API returned {status}: {text}"),
            });
        }

        let json: serde_json::Value =
            response.json().await.map_err(|e| MemoryError::Embedding {
                message: format!("failed to parse embedding response: {e}"),
            })?;

        let data = json["data"]
            .as_array()
            .ok_or_else(|| MemoryError::Embedding {
                message: "embedding response missing 'data' array".to_string(),
            })?;

        let mut result = Vec::with_capacity(data.len());
        for (i, item) in data.iter().enumerate() {
            let raw = item["embedding"]
                .as_array()
                .ok_or_else(|| MemoryError::Embedding {
                    message: format!("embedding item[{i}] missing 'embedding' array"),
                })?;
            let mut embedding = Vec::with_capacity(raw.len());
            for (j, v) in raw.iter().enumerate() {
                let f = v.as_f64().ok_or_else(|| MemoryError::Embedding {
                    message: format!("embedding[{i}][{j}]: non-numeric value {v}"),
                })? as f32;
                if f.is_nan() || f.is_infinite() {
                    return Err(MemoryError::Embedding {
                        message: format!("embedding[{i}][{j}]: invalid float value"),
                    });
                }
                embedding.push(f);
            }
            result.push(embedding);
        }

        Ok(result)
    }
}
