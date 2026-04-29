use async_trait::async_trait;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

pub type ChannelResult<T> = Result<T, ChannelError>;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ChannelError {
    #[error("invalid channel config: {message}")]
    Config { message: String },
    #[error("invalid channel event: {message}")]
    InvalidEvent { message: String },
    #[error("channel authentication failed: {message}")]
    Authentication { message: String },
    #[error("channel transport failed: {message}")]
    Transport { message: String },
    #[error("channel delivery failed: {message}")]
    Delivery { message: String },
    #[error("unsupported channel capability: {capability}")]
    UnsupportedCapability { capability: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMention {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelAttachment {
    pub kind: String,
    pub file_name: String,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelOutboundAttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelOutboundAttachment {
    pub kind: ChannelOutboundAttachmentKind,
    pub path: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMember {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMessage {
    pub channel: String,
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    pub conversation_id: String,
    pub sender_id: String,
    pub message_id: String,
    pub text: String,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub root_message_id: Option<String>,
    #[serde(default)]
    pub mentions: Vec<ChannelMention>,
    #[serde(default)]
    pub attachments: Vec<ChannelAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AdapterResponse {
    Accepted,
    Challenge { challenge: String },
    CustomJson { body: Value },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelProgressState {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelProgressSection {
    pub heading: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelProgressUpdate {
    pub title: String,
    pub summary: String,
    pub state: ChannelProgressState,
    #[serde(default)]
    pub sections: Vec<ChannelProgressSection>,
}

#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn channel_name(&self) -> &str;

    async fn handle_event(
        &self,
        headers: &HeaderMap,
        query: &HashMap<String, String>,
        body: &[u8],
    ) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)>;

    async fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
        reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>>;

    async fn send_attachment(
        &self,
        _conversation_id: &str,
        _attachment: &ChannelOutboundAttachment,
        _reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        Ok(None)
    }

    async fn acknowledge_message(&self, _message_id: &str) -> ChannelResult<()> {
        Ok(())
    }

    async fn add_reaction(&self, _message_id: &str, _emoji_type: &str) -> ChannelResult<()> {
        Ok(())
    }

    async fn list_members(&self, _conversation_id: &str) -> ChannelResult<Vec<ChannelMember>> {
        Ok(Vec::new())
    }

    async fn send_progress_update(
        &self,
        _conversation_id: &str,
        _progress: &ChannelProgressUpdate,
        _reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        Ok(None)
    }

    async fn update_progress_update(
        &self,
        _progress_message_id: &str,
        _progress: &ChannelProgressUpdate,
    ) -> ChannelResult<()> {
        Ok(())
    }

    fn format_user_reference(&self, _user_id: &str) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMeta {
    pub id: String,
    pub label: String,
    pub selection_label: String,
    pub docs_path: String,
    pub docs_label: String,
    pub blurb: String,
    pub aliases: Vec<String>,
    pub order: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelTextFormat {
    PlainText,
    FlattenMarkdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelCapabilities {
    pub supports_webhook: bool,
    pub supports_direct_messages: bool,
    pub supports_group_messages: bool,
    pub requires_async_processing: bool,
    pub supports_threads: bool,
    pub supports_media: bool,
    pub supports_member_listing: bool,
    pub supports_reactions: bool,
    pub supports_progress_updates: bool,
    pub text_reply_format: ChannelTextFormat,
}

#[derive(Clone)]
pub struct ChannelRuntime {
    pub instance_id: String,
    pub channel_id: String,
    pub meta: ChannelMeta,
    pub capabilities: ChannelCapabilities,
    pub adapter: Arc<dyn ChannelAdapter>,
}
