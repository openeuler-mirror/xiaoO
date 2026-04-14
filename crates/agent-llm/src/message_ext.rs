use agent_types::{ChatMessage, ContentBlock, MessageRole};

pub trait MessageRoleExt {
    fn as_str(&self) -> &'static str;
}

impl MessageRoleExt for MessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

pub trait ChatMessageExt {
    fn new(
        role: MessageRole,
        blocks: Vec<ContentBlock>,
        message_id: Option<String>,
        timestamp_ms: u64,
        api_usage_tokens: Option<usize>,
    ) -> Self
    where
        Self: Sized;

    fn user(text: impl Into<String>) -> Self
    where
        Self: Sized;

    fn system(text: impl Into<String>) -> Self
    where
        Self: Sized;

    fn assistant(text: impl Into<String>, timestamp_ms: u64) -> Self
    where
        Self: Sized;

    fn text(role: MessageRole, text: impl Into<String>, timestamp_ms: u64) -> Self
    where
        Self: Sized;

    fn tool_result(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
        timestamp_ms: u64,
    ) -> Self
    where
        Self: Sized;

    fn text_content(&self) -> Option<&str>;

    fn tool_use_blocks(&self) -> Box<dyn Iterator<Item = (&str, &str, &serde_json::Value)> + '_>;
}

impl ChatMessageExt for ChatMessage {
    fn new(
        role: MessageRole,
        blocks: Vec<ContentBlock>,
        message_id: Option<String>,
        timestamp_ms: u64,
        api_usage_tokens: Option<usize>,
    ) -> Self {
        Self {
            role,
            blocks,
            message_id,
            timestamp_ms,
            api_usage_tokens,
        }
    }

    fn user(text: impl Into<String>) -> Self {
        Self::text(MessageRole::User, text, 0)
    }

    fn system(text: impl Into<String>) -> Self {
        Self::text(MessageRole::System, text, 0)
    }

    fn assistant(text: impl Into<String>, timestamp_ms: u64) -> Self {
        Self::text(MessageRole::Assistant, text, timestamp_ms)
    }

    fn text(role: MessageRole, text: impl Into<String>, timestamp_ms: u64) -> Self {
        Self::new(
            role,
            vec![ContentBlock::Text { text: text.into() }],
            None,
            timestamp_ms,
            None,
        )
    }

    fn tool_result(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
        timestamp_ms: u64,
    ) -> Self {
        Self::new(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                call_id: call_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
            }],
            None,
            timestamp_ms,
            None,
        )
    }

    fn text_content(&self) -> Option<&str> {
        self.blocks.iter().find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
    }

    fn tool_use_blocks(&self) -> Box<dyn Iterator<Item = (&str, &str, &serde_json::Value)> + '_> {
        Box::new(self.blocks.iter().filter_map(|block| match block {
            ContentBlock::ToolUse {
                call_id,
                tool_name,
                input,
            } => Some((call_id.as_str(), tool_name.as_str(), input)),
            _ => None,
        }))
    }
}
