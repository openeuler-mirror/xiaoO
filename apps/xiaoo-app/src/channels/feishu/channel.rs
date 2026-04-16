use crate::channels::feishu::client::{build_progress_card_content, FeishuClient};
use crate::channels::feishu::ingress::handle_event as handle_feishu_event;
use crate::channels::feishu::types::{
    FeishuCardRequest, FeishuChatInfo, FeishuConfig, FeishuSendRequest,
};
use crate::channels::{
    AdapterResponse, ChannelAdapter, ChannelCapabilities, ChannelError, ChannelMember,
    ChannelMessage, ChannelMeta, ChannelOutboundAttachment,
    ChannelProgressUpdate, ChannelResult, ChannelTextFormat,
};
use async_trait::async_trait;
use axum::http::HeaderMap;

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

    pub async fn get_chat_info(&self, chat_id: &str) -> ChannelResult<FeishuChatInfo> {
        self.client.get_chat_info(chat_id).await
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

        let file_key = self
            .client
            .upload_file(&attachment.path, file_name)
            .await?;

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
mod tests {
    use super::*;

    fn config() -> FeishuConfig {
        FeishuConfig {
            channel_instance_id: Some("ops-feishu".to_string()),
            base_url: "https://open.feishu.cn".to_string(),
            app_id: "cli_test".to_string(),
            app_secret_env: "FEISHU_APP_SECRET".to_string(),
            verification_token: "verify_token".to_string(),
            parse_file_messages: false,
            max_file_download_bytes: 0,
            max_file_text_chars: 0,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handles_url_verification_challenge() {
        let adapter = FeishuAdapter::new(config()).expect("valid config");
        let body = serde_json::json!({
            "type": "url_verification",
            "challenge": "challenge-token",
            "token": "verify_token"
        })
        .to_string();

        let (response, message) = adapter
            .handle_event(&HeaderMap::new(), body.as_bytes())
            .await
            .expect("challenge should succeed");

        assert_eq!(
            response,
            AdapterResponse::Challenge {
                challenge: "challenge-token".to_string()
            }
        );
        assert!(message.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn parses_reply_chain_metadata() {
        let adapter = FeishuAdapter::new(config()).expect("valid config");
        let body = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "verify_token"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_current",
                    "chat_id": "oc_test",
                    "root_id": "om_root",
                    "parent_id": "om_parent",
                    "message_type": "text",
                    "content": "{\"text\":\"<at user_id=\\\"ou_target\\\">李四</at> 请看这条\"}",
                    "mentions": [{
                        "id": {
                            "open_id": "ou_target"
                        },
                        "name": "李四"
                    }]
                }
            }
        })
        .to_string();

        let (response, message) = adapter
            .handle_event(&HeaderMap::new(), body.as_bytes())
            .await
            .expect("message should succeed");

        assert_eq!(response, AdapterResponse::Accepted);
        let message = message.expect("message should exist");
        assert_eq!(message.message_id, "om_current");
        assert_eq!(message.reply_to_message_id.as_deref(), Some("om_parent"));
        assert_eq!(message.root_message_id.as_deref(), Some("om_root"));
        assert_eq!(message.mentions.len(), 1);
        assert_eq!(message.mentions[0].id, "ou_target");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn normalizes_empty_reply_chain_metadata_to_none() {
        let adapter = FeishuAdapter::new(config()).expect("valid config");
        let body = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "verify_token"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_top_level",
                    "chat_id": "oc_test",
                    "root_id": "",
                    "parent_id": "   ",
                    "message_type": "text",
                    "content": "{\"text\":\"你好\"}"
                }
            }
        })
        .to_string();

        let (response, message) = adapter
            .handle_event(&HeaderMap::new(), body.as_bytes())
            .await
            .expect("message should succeed");

        assert_eq!(response, AdapterResponse::Accepted);
        let message = message.expect("message should exist");
        assert_eq!(message.message_id, "om_top_level");
        assert!(message.reply_to_message_id.is_none());
        assert!(message.root_message_id.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn strips_repeated_leading_invocation_mentions() {
        let adapter = FeishuAdapter::new(config()).expect("valid config");
        let body = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "verify_token"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_repeat",
                    "chat_id": "oc_test",
                    "message_type": "text",
                    "content": "{\"text\":\"@@_user_1 现在拥有哪些能力\"}",
                    "mentions": [{
                        "key": "@_user_1",
                        "id": {
                            "open_id": "ou_bot"
                        },
                        "name": "小欧 beta 0.92"
                    }]
                }
            }
        })
        .to_string();

        let (_response, message) = adapter
            .handle_event(&HeaderMap::new(), body.as_bytes())
            .await
            .expect("message should succeed");

        let message = message.expect("message should exist");
        assert_eq!(message.text, "现在拥有哪些能力");
        assert_eq!(message.mentions.len(), 1);
        assert_eq!(message.mentions[0].id, "ou_bot");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejects_invalid_verification_token() {
        let adapter = FeishuAdapter::new(config()).expect("valid config");
        let body = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "wrong"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_current",
                    "chat_id": "oc_test",
                    "message_type": "text",
                    "content": "{\"text\":\"hello\"}"
                }
            }
        })
        .to_string();

        let error = adapter
            .handle_event(&HeaderMap::new(), body.as_bytes())
            .await
            .expect_err("message should fail");

        assert!(matches!(error, ChannelError::Authentication { .. }));
    }
}
