use crate::wire_types::{WireResponseFormat, WireToolCall, WireToolCallFunction, WireToolChoice};
use agent_types::Tool;

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
}
