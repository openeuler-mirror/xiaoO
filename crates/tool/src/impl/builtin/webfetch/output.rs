use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebFetchOutput {
    pub content: String,
    pub url: String,
    pub content_type: String,
    pub format: String,
}
