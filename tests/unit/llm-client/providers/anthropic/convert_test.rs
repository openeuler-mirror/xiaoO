use super::*;
use agent_llm::ChatMessageExt;
use agent_types::MessageRole;

#[test]
fn test_to_anthropic_tool() {
    let tool = Tool {
        name: "get_weather".to_string(),
        description: "Get weather info".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "location": {"type": "string"} }
        }),
    };
    let anthropic_tool = to_anthropic_tool(&tool);
    assert_eq!(anthropic_tool["name"], "get_weather");
    assert_eq!(anthropic_tool["description"], "Get weather info");
    assert!(anthropic_tool.get("input_schema").is_some());
}

#[test]
fn test_to_anthropic_tool_choice_auto() {
    let choice = WireToolChoice::auto();
    let result = to_anthropic_tool_choice(&choice);
    assert_eq!(result["type"], "auto");
}

#[test]
fn test_to_anthropic_tool_choice_required() {
    let choice = WireToolChoice::required();
    let result = to_anthropic_tool_choice(&choice);
    assert_eq!(result["type"], "any");
}

#[test]
fn test_to_anthropic_tool_choice_function() {
    let choice = WireToolChoice::function("get_weather".to_string());
    let result = to_anthropic_tool_choice(&choice);
    assert_eq!(result["type"], "tool");
    assert_eq!(result["name"], "get_weather");
}

#[test]
fn test_extract_anthropic_tool_calls() {
    let content = serde_json::json!([{
        "type": "tool_use",
        "id": "toolu_123",
        "name": "get_weather",
        "input": {"location": "Tokyo"}
    }]);
    let tool_calls = extract_anthropic_tool_calls(&content);
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "toolu_123");
    assert_eq!(tool_calls[0].function.name, "get_weather");
}

#[test]
fn test_anthropic_messages_convert_assistant_tool_use() {
    let msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![
            ContentBlock::Text {
                text: "Let me check".to_string(),
            },
            ContentBlock::ToolUse {
                call_id: "toolu_123".to_string(),
                tool_name: "bash".to_string(),
                input: serde_json::json!({"command": "date"}),
            },
        ],
        None,
        0,
        None,
    );

    let messages = anthropic_messages(&[msg]);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["content"][0]["type"], "text");
    assert_eq!(messages[0]["content"][1]["type"], "tool_use");
    assert_eq!(messages[0]["content"][1]["id"], "toolu_123");
    assert_eq!(messages[0]["content"][1]["name"], "bash");
}

#[test]
fn test_anthropic_messages_convert_tool_result_to_user_block() {
    let msg = ChatMessage::tool_result("toolu_123", "bash", "done", false, 0);

    let messages = anthropic_messages(&[msg]);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["type"], "tool_result");
    assert_eq!(messages[0]["content"][0]["tool_use_id"], "toolu_123");
    assert_eq!(messages[0]["content"][0]["content"], "done");
}

#[test]
fn test_anthropic_system_blocks_one_entry_per_message() {
    let messages = vec![
        ChatMessage::system("base system"),
        ChatMessage::new(
            MessageRole::System,
            vec![ContentBlock::Document {
                description: "workspace doc".to_string(),
            }],
            None,
            0,
            None,
        ),
    ];

    let blocks = anthropic_system_blocks(&messages);
    assert_eq!(blocks, vec!["base system", "workspace doc"]);
}
