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
#[path = "../../../../tests/unit/llm-client/wire_types/tool_test.rs"]
mod tests;
