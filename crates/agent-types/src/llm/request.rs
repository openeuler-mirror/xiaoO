use crate::llm::ChatMessage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
    pub max_tokens: Option<usize>,
    pub temperature: Option<f64>,
    pub response_format: ResponseFormat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    Required,
    None,
    Specific(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    #[default]
    Text,
    JsonObject,
    JsonSchema {
        #[serde(default)]
        name: String,
        schema: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompletionConfig {
    pub max_tokens: usize,
    pub temperature: f64,
}
