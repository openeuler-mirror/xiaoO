use crate::channels::feishu::client::{build_progress_card_content, FeishuClient};
use crate::channels::feishu::ingress::handle_event as handle_feishu_event;
use crate::channels::feishu::types::{FeishuCardRequest, FeishuConfig, FeishuSendRequest};
use crate::channels::{
    AdapterResponse, ChannelAdapter, ChannelCapabilities, ChannelError, ChannelMember,
    ChannelMessage, ChannelMeta, ChannelOutboundAttachment, ChannelProgressUpdate, ChannelResult,
    ChannelTextFormat,
};
use async_trait::async_trait;
use axum::http::HeaderMap;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FeishuAdapter {
    config: FeishuConfig,
    client: FeishuClient,
}

const ACK_REACTION_EMOJI_TYPE: &str = "Get";

impl FeishuAdapter {
    pub fn new(config: FeishuConfig) -> ChannelResult<Self> {
        config.validate().map_err(|error| ChannelError::Config {
            message: error.to_string(),
        })?;
        Ok(Self {
            client: FeishuClient::new(config.clone()),
            config,
        })
    }
}

#[async_trait]
impl ChannelAdapter for FeishuAdapter {
    fn channel_name(&self) -> &str {
        "feishu"
    }

    async fn handle_event(
        &self,
        _headers: &HeaderMap,
        _query: &HashMap<String, String>,
        body: &[u8],
    ) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
        handle_feishu_event(&self.config, body)
    }

    async fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
        reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        self.client
            .send_text(&FeishuSendRequest {
                conversation_id: conversation_id.to_string(),
                reply_to_message_id: reply_to_message_id.map(|value| value.to_string()),
                text: text.to_string(),
            })
            .await
    }

    async fn acknowledge_message(&self, message_id: &str) -> ChannelResult<()> {
        self.client
            .add_reaction(message_id, ACK_REACTION_EMOJI_TYPE)
            .await
    }

    async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> ChannelResult<()> {
        self.client.add_reaction(message_id, emoji_type).await
    }

    async fn list_members(&self, conversation_id: &str) -> ChannelResult<Vec<ChannelMember>> {
        Ok(self
            .client
            .list_members(conversation_id)
            .await?
            .into_iter()
            .map(|member| ChannelMember {
                id: member.id,
                display_name: member.name,
            })
            .collect())
    }

    async fn send_progress_update(
        &self,
        conversation_id: &str,
        progress: &ChannelProgressUpdate,
        reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        self.client
            .send_progress_card(&FeishuCardRequest {
                conversation_id: conversation_id.to_string(),
                reply_to_message_id: reply_to_message_id.map(|value| value.to_string()),
                content: build_progress_card_content(progress)?,
            })
            .await
    }

    async fn update_progress_update(
        &self,
        progress_message_id: &str,
        progress: &ChannelProgressUpdate,
    ) -> ChannelResult<()> {
        self.client
            .update_progress_card(progress_message_id, progress)
            .await
    }

    async fn send_attachment(
        &self,
        conversation_id: &str,
        attachment: &ChannelOutboundAttachment,
        reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        let file_name = std::path::Path::new(&attachment.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");

        let file_key = self.client.upload_file(&attachment.path, file_name).await?;

        self.client
            .send_file_message(conversation_id, &file_key, reply_to_message_id)
            .await
    }

    fn format_user_reference(&self, user_id: &str) -> Option<String> {
        Some(format!(r#"<at user_id="{user_id}">你</at>"#))
    }
}

pub fn meta() -> ChannelMeta {
    ChannelMeta {
        id: "feishu".to_string(),
        label: "Feishu".to_string(),
        selection_label: "Feishu/Lark".to_string(),
        docs_path: "/channels/feishu".to_string(),
        docs_label: "feishu".to_string(),
        blurb: "Feishu/Lark enterprise messaging webhook adapter.".to_string(),
        aliases: vec!["lark".to_string()],
        order: 70,
    }
}

pub fn capabilities() -> ChannelCapabilities {
    ChannelCapabilities {
        supports_webhook: true,
        supports_direct_messages: true,
        supports_group_messages: true,
        requires_async_processing: true,
        supports_threads: true,
        supports_media: true,
        supports_member_listing: true,
        supports_reactions: true,
        supports_progress_updates: true,
        text_reply_format: ChannelTextFormat::FlattenMarkdown,
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/serverside/channels/feishu/channel_test.rs"]
mod tests;
