use agent_types::{ChatMessage, ContentBlock as ChatContentBlock, MessageRole};
use serde::{Deserialize, Serialize};

use crate::{
    ensure_valid_session_id, FactMemory, InstructionMemory, MemoryResult, PromptHistoryEntry,
    SessionMemorySummary, TaskMemory, TokenUsageBaseline,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversationMessage {
    pub role: MemoryRole,
    pub blocks: Vec<ContentBlock>,
    pub message_id: Option<String>,
    pub timestamp_ms: u64,
    pub api_usage_tokens: Option<usize>,
}

impl From<&ChatMessage> for ConversationMessage {
    fn from(message: &ChatMessage) -> Self {
        let role = match message.role {
            MessageRole::System => MemoryRole::System,
            MessageRole::User => MemoryRole::User,
            MessageRole::Assistant => MemoryRole::Assistant,
            MessageRole::Tool => MemoryRole::Tool,
        };

        let blocks = message
            .blocks
            .iter()
            .map(|block| match block {
                ChatContentBlock::Text { text } => ContentBlock::Text { text: text.clone() },
                ChatContentBlock::ToolUse {
                    call_id,
                    tool_name,
                    input,
                } => ContentBlock::ToolUse {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    input: input.clone(),
                },
                ChatContentBlock::ToolResult {
                    call_id,
                    tool_name,
                    output,
                    is_error,
                } => ContentBlock::ToolResult {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    output: output.clone(),
                    is_error: *is_error,
                },
                ChatContentBlock::Image { description } => ContentBlock::Image {
                    description: description.clone(),
                },
                ChatContentBlock::Document { description } => ContentBlock::Document {
                    description: description.clone(),
                },
            })
            .collect();

        Self {
            role,
            blocks,
            message_id: message.message_id.clone(),
            timestamp_ms: message.timestamp_ms,
            api_usage_tokens: message.api_usage_tokens,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemorySnapshot {
    pub session_id: String,
    pub updated_at: u64,
    pub messages: Vec<ChatMessage>,
    /// Derived view of `messages` — not serialized, rebuilt on construction and deserialization.
    #[serde(skip)]
    pub conversation: Vec<ConversationMessage>,
    pub instructions: Vec<InstructionMemory>,
    pub facts: Vec<FactMemory>,
    pub task: Option<TaskMemory>,
    pub prompt_history: Vec<PromptHistoryEntry>,
    pub usage_baseline: Option<TokenUsageBaseline>,
    pub session_memory: Option<SessionMemorySummary>,
}

impl MemorySnapshot {
    pub fn new(
        session_id: impl Into<String>,
        updated_at: u64,
        messages: Vec<ChatMessage>,
    ) -> MemoryResult<Self> {
        let session_id = session_id.into();
        ensure_valid_session_id(&session_id)?;

        let conversation = messages.iter().map(ConversationMessage::from).collect();

        Ok(Self {
            session_id,
            updated_at,
            messages,
            conversation,
            instructions: Vec::new(),
            facts: Vec::new(),
            task: None,
            prompt_history: Vec::new(),
            usage_baseline: None,
            session_memory: None,
        })
    }

    /// Rebuilds the derived `conversation` field from `messages`.
    /// Must be called after deserialization since `conversation` is serde-skipped.
    pub fn rebuild_conversation(&mut self) {
        self.conversation = self
            .messages
            .iter()
            .map(ConversationMessage::from)
            .collect();
    }

    pub fn sync_messages(&mut self, messages: &[ChatMessage], updated_at: u64) {
        self.messages = messages.to_vec();
        self.conversation = messages.iter().map(ConversationMessage::from).collect();
        self.updated_at = updated_at;
    }
}
