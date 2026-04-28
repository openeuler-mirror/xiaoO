use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        call_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
    },
    Image {
        description: String,
    },
    Document {
        description: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub message_id: Option<String>,
    pub timestamp_ms: u64,
    pub api_usage_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}
