use super::*;
use agent_llm::{ChatMessageExt, LlmRequestExt};
use agent_types::LlmRequest;

#[test]
fn test_sanitize_gemini_schema_uppercases_types() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "items": { "type": "array", "items": {"type": "integer"} }
        }
    });
    let converted = sanitize_gemini_schema(&schema, GeminiSchemaMode::ResponseSchema);
    assert_eq!(converted["type"], "OBJECT");
    assert_eq!(converted["properties"]["name"]["type"], "STRING");
    assert_eq!(converted["properties"]["items"]["type"], "ARRAY");
    assert_eq!(converted["properties"]["items"]["items"]["type"], "INTEGER");
}

#[test]
fn test_sanitize_gemini_schema_removes_additional_properties() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "name": {"type": "string"} },
        "additionalProperties": false
    });
    let converted = sanitize_gemini_schema(&schema, GeminiSchemaMode::ResponseSchema);
    assert!(converted.get("additionalProperties").is_none());
}

#[test]
fn test_normalize_model_name() {
    assert_eq!(normalize_model_name("gemini-pro"), "models/gemini-pro");
    assert_eq!(
        normalize_model_name("models/gemini-pro"),
        "models/gemini-pro"
    );
}

#[test]
fn build_gemini_request_body_concatenates_multiple_system_messages() {
    let request = LlmRequest {
        messages: vec![
            agent_types::ChatMessage::system("base system"),
            agent_types::ChatMessage::system("workspace rules"),
            agent_types::ChatMessage::user("hello"),
        ],
        tools: Vec::new(),
        tool_choice: agent_types::ToolChoice::Auto,
        max_tokens: None,
        temperature: None,
        response_format: agent_types::ResponseFormat::Text,
        reasoning_effort: agent_types::ReasoningEffort::Off,
    };

    let body = build_gemini_request_body(&request, "gemini-pro");

    let system_instruction = body.system_instruction.expect("system instruction");
    assert_eq!(system_instruction.parts.len(), 1);
    assert_eq!(
        system_instruction.parts[0].text.as_deref(),
        Some("base system\n\nworkspace rules")
    );
    assert_eq!(body.contents.len(), 1);
}

#[test]
fn build_gemini_request_body_sets_thinking_budget() {
    let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
    request.reasoning_effort = agent_types::ReasoningEffort::High;

    let body = build_gemini_request_body(&request, "gemini-pro");

    assert_eq!(
        body.generation_config
            .thinking_config
            .unwrap()
            .thinking_budget,
        8192
    );
}

#[test]
fn build_gemini_request_body_omits_thinking_config_when_off() {
    let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
    request.reasoning_effort = agent_types::ReasoningEffort::Off;

    let body = build_gemini_request_body(&request, "gemini-pro");

    assert!(body.generation_config.thinking_config.is_none());
}
