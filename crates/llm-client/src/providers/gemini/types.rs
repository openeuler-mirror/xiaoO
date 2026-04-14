use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(crate) struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    pub function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    pub function_response: Option<serde_json::Value>,
}

impl GeminiPart {
    pub(crate) fn text(text: String) -> Self {
        Self {
            text: Some(text),
            function_call: None,
            function_response: None,
        }
    }
    pub(crate) fn function_call(name: String, args: serde_json::Value) -> Self {
        Self {
            text: None,
            function_call: Some(GeminiFunctionCall { name, args }),
            function_response: None,
        }
    }
    pub(crate) fn function_response(name: String, response: String) -> Self {
        Self {
            text: None,
            function_call: None,
            function_response: Some(
                serde_json::json!({ "name": name, "response": { "content": response } }),
            ),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxOutputTokens")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "responseMimeType")]
    pub response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "responseJsonSchema")]
    pub response_json_schema: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GeminiFunctionDeclaration {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct GeminiFunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct GeminiRequestBody {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(rename = "generationConfig")]
    pub generation_config: GeminiGenerationConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "toolConfig")]
    pub tool_config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiResponseBody {
    #[serde(default)]
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default, rename = "usageMetadata")]
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiCandidate {
    #[serde(default)]
    pub content: Option<GeminiCandidateContent>,
    #[serde(default, rename = "finishReason")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiCandidateContent {
    #[serde(default)]
    pub parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiResponsePart {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "functionCall")]
    pub function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    pub prompt_token_count: Option<u32>,
    #[serde(default, rename = "candidatesTokenCount")]
    pub candidates_token_count: Option<u32>,
}
