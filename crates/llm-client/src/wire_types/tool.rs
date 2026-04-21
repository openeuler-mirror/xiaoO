use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: WireToolFunction,
}

#[allow(dead_code)]
impl WireTool {
    #[allow(dead_code)]
    pub(crate) fn function(
        name: String,
        description: String,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: WireToolFunction {
                name,
                description: Some(description),
                parameters: Some(parameters),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum WireToolChoice {
    String(String),
    Function {
        r#type: String,
        function: FunctionChoice,
    },
}

impl WireToolChoice {
    pub(crate) fn none() -> Self {
        WireToolChoice::String("none".to_string())
    }

    pub(crate) fn auto() -> Self {
        WireToolChoice::String("auto".to_string())
    }

    pub(crate) fn required() -> Self {
        WireToolChoice::String("required".to_string())
    }

    pub(crate) fn function(name: String) -> Self {
        WireToolChoice::Function {
            r#type: "function".to_string(),
            function: FunctionChoice { name },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FunctionChoice {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: WireToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<WireToolCallFunctionDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireToolCallFunctionDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_serialization() {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "location": {"type": "string"}
            }
        });
        let tool = WireTool::function(
            "get_weather".to_string(),
            "Get weather for a location".to_string(),
            params,
        );

        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains(r#""type":"function""#));
        assert!(json.contains(r#""name":"get_weather""#));
        assert!(json.contains(r#""description":"Get weather for a location""#));
    }

    #[test]
    fn test_tool_choice_serialization() {
        let choice = WireToolChoice::auto();
        let json = serde_json::to_string(&choice).unwrap();
        assert_eq!(json, r#""auto""#);

        let choice = WireToolChoice::none();
        let json = serde_json::to_string(&choice).unwrap();
        assert_eq!(json, r#""none""#);

        let choice = WireToolChoice::required();
        let json = serde_json::to_string(&choice).unwrap();
        assert_eq!(json, r#""required""#);

        let choice = WireToolChoice::function("get_weather".to_string());
        let json = serde_json::to_string(&choice).unwrap();
        assert!(json.contains(r#""type":"function""#));
        assert!(json.contains(r#""name":"get_weather""#));
    }

    #[test]
    fn test_tool_call_serialization() {
        let tool_call = WireToolCall {
            id: "call_abc123".to_string(),
            call_type: "function".to_string(),
            function: WireToolCallFunction {
                name: "get_weather".to_string(),
                arguments: r#"{"location":"San Francisco"}"#.to_string(),
            },
        };

        let json = serde_json::to_string(&tool_call).unwrap();
        assert!(json.contains(r#""id":"call_abc123""#));
        assert!(json.contains(r#""type":"function""#));
        assert!(json.contains(r#""name":"get_weather""#));
    }

    #[test]
    fn test_tool_call_delta_serialization() {
        let delta = WireToolCallDelta {
            index: 0,
            id: Some("call_abc123".to_string()),
            call_type: Some("function".to_string()),
            function: Some(WireToolCallFunctionDelta {
                name: Some("get_weather".to_string()),
                arguments: Some(r#"{"loc"#.to_string()),
            }),
        };

        let json = serde_json::to_string(&delta).unwrap();
        assert!(json.contains(r#""index":0"#));
        assert!(json.contains(r#""id":"call_abc123""#));
    }
}
