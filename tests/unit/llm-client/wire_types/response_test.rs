use super::*;

// Test-only constructors for `Warning`.
impl Warning {
    fn new(
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

    fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[test]
fn warning_serialization_omits_absent_message() {
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
fn wire_response_serializes_warnings_only_when_present() {
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
            prompt_tokens_details: None,
        },
        warnings: Some(vec![Warning::new(
            "response_format",
            "anthropic",
            "ignored",
        )]),
        kv_transfer_params: None,
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
            prompt_tokens_details: None,
        },
        warnings: None,
        kv_transfer_params: None,
    };

    let json = serde_json::to_string(&response_no_warnings).unwrap();
    assert!(!json.contains(r#""warnings""#));
}
