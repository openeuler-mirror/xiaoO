use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    ContentFilter,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolUseBlock>,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub message: AssistantMessage,
}

#[derive(Clone, Debug)]
pub struct StreamChunk {
    pub delta_text: Option<String>,
    pub delta_tool_call: Option<ToolUseBlock>,
}
