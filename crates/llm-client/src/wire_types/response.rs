use serde::{Deserialize, Serialize};

use super::message::WireMessage;
use super::tool::WireToolCall;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Warning {
    pub feature: String,
    pub provider: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[allow(dead_code)]
impl Warning {
    pub(crate) fn new(
        feature: impl Into<String>,
        provider: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            feature: feature.into(),
            provider: provider.into(),
            action: action.into(),
            message: None,
        }
    }
    #[allow(dead_code)]
    pub(crate) fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<WireChoice>,
    pub usage: WireUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<Warning>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireChoice {
    pub message: WireMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_warning_serialization() {
        let warning = Warning::new("response_format", "anthropic", "ignored");
        let json = serde_json::to_string(&warning).unwrap();
        assert!(json.contains(r#""feature":"response_format""#));
        assert!(json.contains(r#""provider":"anthropic""#));
        assert!(json.contains(r#""action":"ignored""#));
        assert!(!json.contains(r#""message""#));

        let warning = Warning::new("response_format", "anthropic", "ignored")
            .with_message("Provider does not support structured output");
        let json = serde_json::to_string(&warning).unwrap();
        assert!(json.contains(r#""message":"Provider does not support structured output""#));
    }

    #[test]
    fn test_wire_response_with_warnings() {
        let response = WireResponse {
            id: "test-123".to_string(),
            model: "gpt-4o".to_string(),
            choices: vec![WireChoice {
                message: WireMessage::assistant("Hello"),
                finish_reason: Some("stop".to_string()),
                tool_calls: None,
            }],
            usage: WireUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            warnings: Some(vec![Warning::new(
                "response_format",
                "anthropic",
                "ignored",
            )]),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""warnings""#));
        assert!(json.contains(r#""feature":"response_format""#));

        let response_no_warnings = WireResponse {
            id: "test-456".to_string(),
            model: "gpt-4o".to_string(),
            choices: vec![WireChoice {
                message: WireMessage::assistant("Hello"),
                finish_reason: Some("stop".to_string()),
                tool_calls: None,
            }],
            usage: WireUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            warnings: None,
        };

        let json = serde_json::to_string(&response_no_warnings).unwrap();
        assert!(!json.contains(r#""warnings""#));
    }
}
