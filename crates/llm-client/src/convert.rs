use crate::wire_types::{
    ParsedChunk, WireChoice, WireMessage, WireRequest, WireResponse, WireResponseFormat, WireTool,
    WireToolCall, WireToolCallFunction, WireToolChoice, WireToolFunction, WireUsage,
};
use agent_llm::MessageRoleExt;
use agent_types::{AssistantMessage, LlmResponse, StopReason, StreamChunk, ToolUseBlock, Usage};
use agent_types::{ChatMessage, ContentBlock};
use agent_types::{LlmRequest, ResponseFormat, Tool, ToolChoice};
use serde_json::{Map, Value};

pub(crate) fn chat_messages_to_wire(messages: &[ChatMessage]) -> Vec<WireMessage> {
    messages.iter().map(chat_message_to_wire).collect()
}

pub(crate) fn chat_message_to_wire(msg: &ChatMessage) -> WireMessage {
    let role = msg.role.as_str().to_string();

    let mut content: Option<String> = None;
    let mut tool_calls: Option<Vec<WireToolCall>> = None;
    let mut tool_call_id: Option<String> = None;
    let reasoning_content = if msg.role == agent_types::MessageRole::Assistant {
        msg.reasoning_content.clone()
    } else {
        None
    };

    for block in &msg.blocks {
        match block {
            ContentBlock::Text { text } => {
                content = Some(text.clone());
            }
            ContentBlock::ToolUse {
                call_id,
                tool_name,
                input,
            } => {
                let tc = WireToolCall {
                    id: call_id.clone(),
                    call_type: "function".to_string(),
                    function: WireToolCallFunction {
                        name: tool_name.clone(),
                        arguments: input.to_string(),
                    },
                };
                tool_calls.get_or_insert_with(Vec::new).push(tc);
            }
            ContentBlock::ToolResult {
                call_id, output, ..
            } => {
                tool_call_id = Some(call_id.clone());
                content = Some(output.clone());
            }
            ContentBlock::Image { description } | ContentBlock::Document { description } => {
                content = Some(description.clone());
            }
        }
    }

    WireMessage {
        role,
        content,
        reasoning_content,
        tool_calls,
        tool_call_id,
    }
}

pub(crate) fn llm_request_to_wire(request: &LlmRequest, model: &str) -> WireRequest {
    let messages = chat_messages_to_wire(&request.messages);

    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(request.tools.iter().map(tool_to_wire).collect())
    };

    // Only set tool_choice when tools are present (some APIs reject tool_choice without tools)
    let tool_choice = tools
        .as_ref()
        .map(|_| tool_choice_to_wire(&request.tool_choice));

    let response_format = response_format_to_wire(&request.response_format);

    WireRequest {
        model: model.to_string(),
        messages,
        temperature: request
            .temperature
            .map(|t| crate::wire_types::Temperature::new(t as f32)),
        max_tokens: request.max_tokens.map(|t| t as u32),
        stream: None,
        tools,
        tool_choice,
        response_format,
        route_info: None,
        extra_fields: None,
    }
}

pub(crate) fn tool_to_wire(tool: &Tool) -> WireTool {
    WireTool {
        tool_type: "function".to_string(),
        function: WireToolFunction {
            name: tool.name.clone(),
            description: Some(tool.description.clone()),
            parameters: Some(normalize_tool_parameters_schema(&tool.parameters)),
        },
    }
}

fn normalize_tool_parameters_schema(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return empty_object_schema();
    };

    if object.get("type").and_then(Value::as_str) == Some("object") {
        return schema.clone();
    }

    let mut normalized = object.clone();
    normalized.insert("type".to_string(), Value::String("object".to_string()));
    normalized
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    Value::Object(normalized)
}

fn empty_object_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

pub(crate) fn tool_choice_to_wire(choice: &ToolChoice) -> WireToolChoice {
    match choice {
        ToolChoice::Auto => WireToolChoice::auto(),
        ToolChoice::Required => WireToolChoice::required(),
        ToolChoice::None => WireToolChoice::none(),
        ToolChoice::Specific(name) => WireToolChoice::function(name.clone()),
    }
}

pub(crate) fn response_format_to_wire(format: &ResponseFormat) -> Option<WireResponseFormat> {
    match format {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(WireResponseFormat::json_object()),
        ResponseFormat::JsonSchema { name, schema } => Some(WireResponseFormat::json_schema(
            if name.is_empty() {
                "response".to_string()
            } else {
                name.clone()
            },
            schema.clone(),
        )),
    }
}

#[inline]
pub(crate) fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Null;
    }

    parse_tool_arguments_once(trimmed).unwrap_or(serde_json::Value::Null)
}

fn parse_tool_arguments_once(arguments: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str(arguments) {
        return Some(normalize_tool_arguments(value));
    }

    let repaired = repair_unclosed_json(arguments)?;
    serde_json::from_str(&repaired)
        .ok()
        .map(normalize_tool_arguments)
}

fn normalize_tool_arguments(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(inner) => parse_tool_arguments(&inner),
        other => other,
    }
}

fn repair_unclosed_json(input: &str) -> Option<String> {
    let mut closers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for ch in input.chars() {
        if in_string {
            match ch {
                '\\' if !escaped => {
                    escaped = true;
                }
                '"' if !escaped => {
                    in_string = false;
                }
                _ => {
                    escaped = false;
                }
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => closers.push('}'),
            '[' => closers.push(']'),
            '}' | ']' => {
                if closers.pop() != Some(ch) {
                    return None;
                }
            }
            _ => {}
        }
    }

    if in_string || closers.is_empty() {
        return None;
    }

    let mut repaired = input.to_string();
    while let Some(closer) = closers.pop() {
        repaired.push(closer);
    }
    Some(repaired)
}

pub(crate) fn wire_response_to_llm_response(wire: &WireResponse) -> LlmResponse {
    let choice = wire.choices.first();
    let message = match choice {
        Some(c) => wire_choice_to_assistant_message(c),
        None => AssistantMessage {
            text: None,
            reasoning_content: None,
            tool_calls: vec![],
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
            stop_reason: StopReason::EndTurn,
        },
    };

    LlmResponse { message }
}

pub(crate) fn wire_choice_to_assistant_message(choice: &WireChoice) -> AssistantMessage {
    let text = choice.message.content.clone();
    let reasoning_content = choice.message.reasoning_content.clone();

    let tool_calls: Vec<ToolUseBlock> = choice
        .tool_calls
        .as_ref()
        .or(choice.message.tool_calls.as_ref())
        .map(|tcs| {
            tcs.iter()
                .map(|tc| ToolUseBlock {
                    call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    input: parse_tool_arguments(&tc.function.arguments),
                })
                .collect()
        })
        .unwrap_or_default();

    let stop_reason = match choice.finish_reason.as_deref() {
        Some("stop") | Some("end_turn") => StopReason::EndTurn,
        Some("length") | Some("max_tokens") => StopReason::MaxTokens,
        Some("tool_calls") | Some("tool_use") => StopReason::ToolUse,
        Some("content_filter") => StopReason::ContentFilter,
        _ => StopReason::EndTurn,
    };

    AssistantMessage {
        text,
        reasoning_content,
        tool_calls,
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
        stop_reason,
    }
}

pub(crate) fn wire_usage_to_usage(wire: &WireUsage) -> Usage {
    Usage {
        prompt_tokens: wire.prompt_tokens as usize,
        completion_tokens: wire.completion_tokens as usize,
        total_tokens: wire.total_tokens as usize,
    }
}

pub(crate) fn parsed_chunk_to_stream_chunk(chunk: &ParsedChunk) -> StreamChunk {
    let delta_tool_call = chunk.tool_calls.as_ref().and_then(|tcs| {
        tcs.first().and_then(|tc| {
            tc.function.as_ref().map(|f| ToolUseBlock {
                call_id: tc.id.clone().unwrap_or_default(),
                tool_name: f.name.clone().unwrap_or_default(),
                input: f
                    .arguments
                    .as_ref()
                    .and_then(|a| serde_json::from_str(a).ok())
                    .unwrap_or(serde_json::Value::Null),
            })
        })
    });

    StreamChunk {
        delta_text: chunk.content.clone(),
        delta_reasoning: chunk.reasoning.clone(),
        delta_tool_call,
    }
}

#[cfg(test)]
mod tests {
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
            },
            warnings: None,
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
}
