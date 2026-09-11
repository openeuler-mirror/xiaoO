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

pub(crate) fn anthropic_system_blocks(messages: &[ChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::System))
        .map(|m| {
            m.blocks
                .iter()
                .filter_map(block_text_for_system)
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .filter(|text| !text.is_empty())
        .collect()
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
            ordered.extend(
                message
                    .blocks
                    .iter()
                    .filter(|block| matches!(block, ContentBlock::ToolResult { .. })),
            );
            ordered.extend(
                message
                    .blocks
                    .iter()
                    .filter(|block| !matches!(block, ContentBlock::ToolResult { .. })),
            );
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

#[cfg(test)]
#[path = "../../../../../tests/unit/llm-client/providers/anthropic/convert_test.rs"]
mod tests;
