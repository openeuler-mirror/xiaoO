use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchFormat {
    Text,
    Markdown,
    Html,
}

impl std::fmt::Display for WebFetchFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebFetchFormat::Text => write!(f, "text"),
            WebFetchFormat::Markdown => write!(f, "markdown"),
            WebFetchFormat::Html => write!(f, "html"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebFetchInput {
    pub url: String,
    pub format: WebFetchFormat,
    #[serde(default)]
    pub timeout: Option<u64>,
}
