use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivecrawlMode {
    Fallback,
    Preferred,
}

impl Default for LivecrawlMode {
    fn default() -> Self {
        LivecrawlMode::Fallback
    }
}

impl std::fmt::Display for LivecrawlMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LivecrawlMode::Fallback => write!(f, "fallback"),
            LivecrawlMode::Preferred => write!(f, "preferred"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchType {
    Auto,
    Fast,
    Deep,
}

impl Default for SearchType {
    fn default() -> Self {
        SearchType::Auto
    }
}

impl std::fmt::Display for SearchType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchType::Auto => write!(f, "auto"),
            SearchType::Fast => write!(f, "fast"),
            SearchType::Deep => write!(f, "deep"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebSearchInput {
    pub query: String,
    #[serde(default)]
    pub num_results: Option<u32>,
    #[serde(default)]
    pub livecrawl: Option<LivecrawlMode>,
    #[serde(default)]
    pub search_type: Option<SearchType>,
    #[serde(default)]
    pub context_max_characters: Option<u32>,
}
