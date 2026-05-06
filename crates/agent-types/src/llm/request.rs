use crate::llm::ChatMessage;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
    pub max_tokens: Option<usize>,
    pub temperature: Option<f64>,
    pub response_format: ResponseFormat,
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    Off,
    High,
    Max,
}

impl ReasoningEffort {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::High,
            Self::High => Self::Max,
            Self::Max => Self::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Ok(Self::Off),
            "high" => Ok(Self::High),
            "max" | "xhigh" | "maximum" => Ok(Self::Max),
            other => Err(format!(
                "invalid reasoning effort '{other}' (expected off, high, or max)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReasoningEffort;
    use std::str::FromStr;

    #[test]
    fn reasoning_effort_cycles_off_high_max() {
        assert_eq!(ReasoningEffort::Off.next(), ReasoningEffort::High);
        assert_eq!(ReasoningEffort::High.next(), ReasoningEffort::Max);
        assert_eq!(ReasoningEffort::Max.next(), ReasoningEffort::Off);
    }

    #[test]
    fn reasoning_effort_parses_aliases() {
        assert_eq!(
            ReasoningEffort::from_str("none").unwrap(),
            ReasoningEffort::Off
        );
        assert_eq!(
            ReasoningEffort::from_str("xhigh").unwrap(),
            ReasoningEffort::Max
        );
    }
}
