use super::*;
use agent_llm::ChatMessageExt;
use agent_types::MessageRole;

#[test]
fn test_chat_message_to_wire_text() {
    let msg = ChatMessage::user("Hello");
    let wire = chat_message_to_wire(&msg);
    assert_eq!(wire.role, "user");
    assert_eq!(wire.content, Some("Hello".to_string()));
    assert!(wire.tool_calls.is_none());
}

#[test]
fn test_chat_message_to_wire_tool_use() {
    let msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![ContentBlock::ToolUse {
            call_id: "call_123".to_string(),
            tool_name: "get_weather".to_string(),
            input: serde_json::json!({"location": "Tokyo"}),
        }],
        None,
        0,
        None,
    );
    let wire = chat_message_to_wire(&msg);
    assert_eq!(wire.role, "assistant");
    assert!(wire.tool_calls.is_some());
    let tcs = wire.tool_calls.unwrap();
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].id, "call_123");
    assert_eq!(tcs[0].function.name, "get_weather");
}

#[test]
fn test_chat_message_to_wire_tool_result() {
    let msg = ChatMessage::tool_result("call_123", "get_weather", "72°F", false, 0);
    let wire = chat_message_to_wire(&msg);
    assert_eq!(wire.role, "tool");
    assert_eq!(wire.tool_call_id, Some("call_123".to_string()));
    assert_eq!(wire.content, Some("72°F".to_string()));
}

#[test]
fn test_chat_message_to_wire_preserves_reasoning_content() {
    let mut msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![ContentBlock::ToolUse {
            call_id: "call_123".to_string(),
            tool_name: "get_weather".to_string(),
            input: serde_json::json!({"location": "Tokyo"}),
        }],
        None,
        0,
        None,
    );
    msg.reasoning_content = Some("thinking trace".to_string());

    let wire = chat_message_to_wire(&msg);

    assert_eq!(wire.role, "assistant");
    assert_eq!(wire.reasoning_content, Some("thinking trace".to_string()));
}

#[test]
fn test_chat_message_to_wire_drops_reasoning_on_text_only_rounds() {
    let mut msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![ContentBlock::Text {
            text: "plain answer".to_string(),
        }],
        None,
        0,
        None,
    );
    msg.reasoning_content = Some("thinking trace".to_string());

    let wire = chat_message_to_wire(&msg);

    assert_eq!(wire.role, "assistant");
    assert_eq!(
        wire.reasoning_content, None,
        "text-only rounds must not forward the reasoning trace"
    );
}

#[test]
fn test_wire_choice_preserves_reasoning_content() {
    let choice = WireChoice {
        message: WireMessage {
            role: "assistant".to_string(),
            content: None,
            reasoning_content: Some("thinking trace".to_string()),
            tool_calls: Some(vec![WireToolCall {
                id: "call_123".to_string(),
                call_type: "function".to_string(),
                function: WireToolCallFunction {
                    name: "get_weather".to_string(),
                    arguments: r#"{"location":"Tokyo"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        },
        finish_reason: Some("tool_calls".to_string()),
        tool_calls: None,
    };

    let message = wire_choice_to_assistant_message(&choice);

    assert_eq!(
        message.reasoning_content,
        Some("thinking trace".to_string())
    );
    assert_eq!(message.tool_calls.len(), 1);
}

#[test]
fn test_wire_response_to_llm_response() {
    let wire = WireResponse {
        id: "resp-1".to_string(),
        model: "gpt-4o".to_string(),
        choices: vec![WireChoice {
            message: WireMessage::assistant("Hello world"),
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

    let response = wire_response_to_llm_response(&wire);
    assert_eq!(response.message.text, Some("Hello world".to_string()));
    assert!(response.message.tool_calls.is_empty());
    assert!(matches!(response.message.stop_reason, StopReason::EndTurn));
}

#[test]
fn test_tool_choice_to_wire() {
    assert!(
        matches!(tool_choice_to_wire(&ToolChoice::Auto), WireToolChoice::String(s) if s == "auto")
    );
    assert!(
        matches!(tool_choice_to_wire(&ToolChoice::Required), WireToolChoice::String(s) if s == "required")
    );
    assert!(
        matches!(tool_choice_to_wire(&ToolChoice::None), WireToolChoice::String(s) if s == "none")
    );
}

#[test]
fn tool_to_wire_normalizes_empty_parameters_to_object_schema() {
    let tool = Tool {
        name: "print_hello_world".to_string(),
        description: "prints hello".to_string(),
        parameters: serde_json::json!({}),
    };

    let wire = tool_to_wire(&tool);
    let parameters = wire.function.parameters.expect("parameters should exist");

    assert_eq!(parameters["type"], "object");
    assert!(parameters["properties"].is_object());
}

#[test]
fn tool_to_wire_preserves_existing_object_schema() {
    let tool = Tool {
        name: "search".to_string(),
        description: "search docs".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
    };

    let wire = tool_to_wire(&tool);
    let parameters = wire.function.parameters.expect("parameters should exist");

    assert_eq!(parameters["type"], "object");
    assert_eq!(parameters["properties"]["query"]["type"], "string");
    assert_eq!(parameters["required"][0], "query");
}

#[test]
fn test_parse_tool_arguments_repairs_missing_trailing_brace() {
    let parsed = parse_tool_arguments(
        r#"{"description":"count workspace","task_goal":"Count files","task_context":"Use find","output_schema":{"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]}"#,
    );

    assert_eq!(parsed["description"], "count workspace");
    assert_eq!(parsed["output_schema"]["type"], "object");
    assert_eq!(parsed["output_schema"]["required"][0], "count");
}

#[test]
fn test_chat_message_to_wire_null_arguments_becomes_empty_object() {
    // When the LLM returns a tool call with invalid/empty JSON arguments,
    // parse_tool_arguments returns Value::Null. When this is sent back to
    // the LLM API in the conversation history, "null" is rejected by
    // Dashscope ("function.arguments must be in JSON format").
    // Fix: serialize Null as "{}" (empty JSON object).
    let msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![ContentBlock::ToolUse {
            call_id: "call_null".to_string(),
            tool_name: "spawn_subagent".to_string(),
            input: serde_json::Value::Null,
        }],
        None,
        0,
        None,
    );
    let wire = chat_message_to_wire(&msg);
    let tcs = wire.tool_calls.expect("tool_calls should exist");
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].function.arguments, "{}");
    assert_ne!(tcs[0].function.arguments, "null");
}

#[test]
fn test_chat_message_to_wire_valid_arguments_preserved() {
    let msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![ContentBlock::ToolUse {
            call_id: "call_valid".to_string(),
            tool_name: "bash".to_string(),
            input: serde_json::json!({"command": "ls -la"}),
        }],
        None,
        0,
        None,
    );
    let wire = chat_message_to_wire(&msg);
    let tcs = wire.tool_calls.expect("tool_calls should exist");
    assert_eq!(tcs.len(), 1);
    let parsed: serde_json::Value =
        serde_json::from_str(&tcs[0].function.arguments).expect("should be valid JSON");
    assert_eq!(parsed["command"], "ls -la");
}

#[test]
fn test_end_to_end_null_arguments_round_trip() {
    // Simulate the full flow:
    // 1. LLM returns tool call with broken arguments
    // 2. xiaoo parses → Null
    // 3. xiaoo stores in conversation history
    // 4. xiaoo sends next request with conversation history
    // 5. function.arguments should be "{}" not "null"

    // Empty/blank arguments parse to Null — the case that must round-trip
    // as `"{}"` (not the bare JSON `null` literal) when re-serialized.
    // Note: malformed-but-closable JSON (e.g. `{"a":"b`) is *repaired* by
    // `repair_unclosed_json` into a valid object, so it would NOT reach
    // Null and must not be used to assert the Null path.
    let broken_arguments = "";
    let parsed = parse_tool_arguments(broken_arguments);
    assert!(parsed.is_null(), "blank arguments should parse to Null");

    let msg = ChatMessage::new(
        MessageRole::Assistant,
        vec![ContentBlock::ToolUse {
            call_id: "call_broken".to_string(),
            tool_name: "spawn_subagent".to_string(),
            input: parsed,
        }],
        None,
        0,
        None,
    );
    let wire = chat_message_to_wire(&msg);
    let tcs = wire.tool_calls.expect("tool_calls should exist");
    assert_eq!(tcs[0].function.arguments, "{}");

    // Verify the serialized body would be accepted by Dashscope
    let parsed_back: serde_json::Value =
        serde_json::from_str(&tcs[0].function.arguments).expect("must be valid JSON");
    assert!(parsed_back.is_object(), "must be a JSON object");
}

#[test]
fn test_wire_choice_repairs_tool_arguments() {
    let choice = WireChoice {
        message: WireMessage::assistant(""),
        finish_reason: Some("tool_calls".to_string()),
        tool_calls: Some(vec![WireToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: WireToolCallFunction {
                name: "spawn_subagent".to_string(),
                arguments: r#"{"description":"count workspace","task_goal":"Count files","task_context":"Use find","output_schema":{"type":"object","properties":{"count":{"type":"integer"}},"required":["count"]}"#.to_string(),
            },
        }]),
    };

    let message = wire_choice_to_assistant_message(&choice);
    assert_eq!(message.tool_calls.len(), 1);
    assert_eq!(message.tool_calls[0].tool_name, "spawn_subagent");
    assert_eq!(
        message.tool_calls[0].input["description"],
        "count workspace"
    );
}
