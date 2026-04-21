use serde::{Deserialize, Serialize};

use super::tool::WireToolCall;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[allow(dead_code)]
impl WireMessage {
    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub(crate) fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub(crate) fn assistant_with_tool_calls(tool_calls: Vec<WireToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub(crate) fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_constructors() {
        let user = WireMessage::user("Hello");
        assert_eq!(user.role, "user");
        assert_eq!(user.content, Some("Hello".to_string()));

        let assistant = WireMessage::assistant("Hi there");
        assert_eq!(assistant.role, "assistant");

        let system = WireMessage::system("You are helpful");
        assert_eq!(system.role, "system");

        let tool_result = WireMessage::tool_result("call_123", "result");
        assert_eq!(tool_result.role, "tool");
        assert_eq!(tool_result.tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn test_message_with_tool_calls() {
        let tool_calls = vec![WireToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: super::super::tool::WireToolCallFunction {
                name: "get_weather".to_string(),
                arguments: "{}".to_string(),
            },
        }];

        let msg = WireMessage::assistant_with_tool_calls(tool_calls);
        assert_eq!(msg.role, "assistant");
        assert!(msg.content.is_none());
        assert!(msg.tool_calls.is_some());
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
    }
}
