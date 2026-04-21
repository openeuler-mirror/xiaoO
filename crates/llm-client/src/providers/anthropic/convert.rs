use crate::wire_types::{WireResponseFormat, WireToolCall, WireToolCallFunction, WireToolChoice};
use agent_types::{ChatMessage, ContentBlock, MessageRole, Tool};

pub(crate) fn to_anthropic_tool(tool: &Tool) -> serde_json::Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

pub(crate) fn to_anthropic_tool_choice(tool_choice: &WireToolChoice) -> serde_json::Value {
    match tool_choice {
        WireToolChoice::String(s) => match s.as_str() {
            "auto" => serde_json::json!({"type": "auto"}),
            "none" => serde_json::json!({"type": "auto"}),
            "required" => serde_json::json!({"type": "any"}),
            _ => serde_json::json!({"type": "auto"}),
        },
        WireToolChoice::Function { function, .. } => {
            serde_json::json!({ "type": "tool", "name": function.name })
        }
    }
}

pub(crate) fn extract_anthropic_tool_calls(content: &serde_json::Value) -> Vec<WireToolCall> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"].as_str() == Some("tool_use"))
        .map(|item| WireToolCall {
            id: item["id"].as_str().unwrap_or_default().to_string(),
            call_type: "function".to_string(),
            function: WireToolCallFunction {
                name: item["name"].as_str().unwrap_or_default().to_string(),
                arguments: item["input"].to_string(),
            },
        })
        .collect()
}

pub(crate) fn anthropic_system_message(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::System))
        .flat_map(|m| m.blocks.iter())
        .filter_map(block_text_for_system)
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn anthropic_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter_map(anthropic_message)
        .collect::<Vec<_>>()
}

fn anthropic_message(message: &ChatMessage) -> Option<serde_json::Value> {
    let role = match message.role {
        MessageRole::System => return None,
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "user",
    };

    let content = anthropic_message_content(message);
    Some(serde_json::json!({
        "role": role,
        "content": content,
    }))
}

fn anthropic_message_content(message: &ChatMessage) -> Vec<serde_json::Value> {
    let blocks: Vec<&ContentBlock> = match message.role {
        MessageRole::User | MessageRole::Tool => {
            let mut ordered = Vec::with_capacity(message.blocks.len());
            ordered.extend(message.blocks.iter().filter(|block| {
                matches!(block, ContentBlock::ToolResult { .. })
            }));
            ordered.extend(message.blocks.iter().filter(|block| {
                !matches!(block, ContentBlock::ToolResult { .. })
            }));
            ordered
        }
        _ => message.blocks.iter().collect(),
    };

    blocks
        .into_iter()
        .map(content_block_to_anthropic)
        .collect::<Vec<_>>()
}

fn content_block_to_anthropic(block: &ContentBlock) -> serde_json::Value {
    match block {
        ContentBlock::Text { text } => serde_json::json!({
            "type": "text",
            "text": text,
        }),
        ContentBlock::ToolUse {
            call_id,
            tool_name,
            input,
        } => serde_json::json!({
            "type": "tool_use",
            "id": call_id,
            "name": tool_name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            call_id,
            output,
            is_error,
            ..
        } => serde_json::json!({
            "type": "tool_result",
            "tool_use_id": call_id,
            "content": output,
            "is_error": is_error,
        }),
        ContentBlock::Image { description } | ContentBlock::Document { description } => {
            serde_json::json!({
                "type": "text",
                "text": description,
            })
        }
    }
}

fn block_text_for_system(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text { text } => Some(text.clone()),
        ContentBlock::Image { description } | ContentBlock::Document { description } => {
            Some(description.clone())
        }
        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => None,
    }
}

pub(crate) fn to_anthropic_output_format(
    response_format: &WireResponseFormat,
) -> Option<serde_json::Value> {
    match response_format.format_type.as_str() {
        "json_schema" => response_format.json_schema.as_ref().map(|json_schema| {
            serde_json::json!({
                "type": "json_schema",
                "schema": json_schema.schema,
            })
        }),
        _ => None,
    }
}

#[allow(dead_code)]
pub(crate) fn response_format_warning(
    response_format: &WireResponseFormat,
) -> Option<crate::wire_types::Warning> {
    if to_anthropic_output_format(response_format).is_some() {
        None
    } else {
        Some(
            crate::wire_types::Warning::new("response_format", "anthropic", "ignored")
                .with_message(
                    "Anthropic output_config.format is only mapped for json_schema requests.",
                ),
        )
    }
}

#[cfg(test)]
mod tests {
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
    fn test_anthropic_system_message_joins_text_blocks() {
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

        let system = anthropic_system_message(&messages);
        assert_eq!(system, "base system\n\nworkspace doc");
    }
}
