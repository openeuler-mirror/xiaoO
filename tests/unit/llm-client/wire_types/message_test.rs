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
