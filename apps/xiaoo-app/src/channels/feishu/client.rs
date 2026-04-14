use crate::channels::feishu::types::{
    ApiResponse, ChatInfoEnvelope, FeishuCardRequest, FeishuChatInfo, FeishuChatMember,
    FeishuConfig, FeishuSendRequest, FeishuTextContent, ReactionRequest, ReactionType,
    ReplyMessageRequest, SendMessageRequest, SendMessageResponse, TenantTokenRequest,
    TenantTokenResponse, UpdateMessageRequest,
};
use crate::channels::{
    ChannelError, ChannelProgressSection, ChannelProgressState, ChannelProgressUpdate,
    ChannelResult,
};
use reqwest::{Client, StatusCode};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct FeishuClient {
    config: FeishuConfig,
    client: Client,
}

impl FeishuClient {
    pub fn new(config: FeishuConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    pub async fn send_text(&self, request: &FeishuSendRequest) -> ChannelResult<Option<String>> {
        let token = self.fetch_tenant_access_token().await?;
        let content = serde_json::to_string(&FeishuTextContent {
            text: request.text.clone(),
        })
        .map_err(|error| ChannelError::Delivery {
            message: format!("failed to serialize feishu text reply: {error}"),
        })?;

        self.send_message_content(
            &token,
            &request.conversation_id,
            request.reply_to_message_id.as_deref(),
            "text",
            content,
            "send_text",
        )
        .await
    }

    pub async fn send_progress_card(
        &self,
        request: &FeishuCardRequest,
    ) -> ChannelResult<Option<String>> {
        let token = self.fetch_tenant_access_token().await?;
        self.send_message_content(
            &token,
            &request.conversation_id,
            request.reply_to_message_id.as_deref(),
            "interactive",
            request.content.clone(),
            "send_progress_card",
        )
        .await
    }

    pub async fn update_progress_card(
        &self,
        message_id: &str,
        progress: &ChannelProgressUpdate,
    ) -> ChannelResult<()> {
        let token = self.fetch_tenant_access_token().await?;
        let response = self
            .client
            .patch(format!(
                "{}/open-apis/im/v1/messages/{message_id}",
                self.config.base_url()
            ))
            .bearer_auth(&token)
            .json(&UpdateMessageRequest {
                msg_type: "interactive",
                content: build_progress_card_content(progress)?,
            })
            .send()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("feishu update_progress_card request failed: {error}"),
            })?;

        ensure_api_success_response(response, "feishu update_progress_card").await
    }

    pub async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> ChannelResult<()> {
        let token = self.fetch_tenant_access_token().await?;
        let response = self
            .client
            .post(format!(
                "{}/open-apis/im/v1/messages/{message_id}/reactions",
                self.config.base_url()
            ))
            .bearer_auth(&token)
            .json(&ReactionRequest {
                reaction_type: ReactionType { emoji_type },
            })
            .send()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("feishu add_reaction request failed: {error}"),
            })?;

        ensure_api_success_response(response, "feishu add_reaction").await
    }

    pub async fn list_members(&self, chat_id: &str) -> ChannelResult<Vec<FeishuChatMember>> {
        let token = self.fetch_tenant_access_token().await?;
        let mut members = Vec::new();
        let mut next_page_token: Option<String> = None;

        loop {
            let mut request = self
                .client
                .get(format!(
                    "{}/open-apis/im/v1/chats/{chat_id}/members",
                    self.config.base_url()
                ))
                .bearer_auth(&token)
                .query(&[("page_size", "50"), ("member_id_type", "open_id")]);

            if let Some(page_token) = next_page_token.as_deref() {
                request = request.query(&[("page_token", page_token)]);
            }

            let response = request
                .send()
                .await
                .map_err(|error| ChannelError::Transport {
                    message: format!("feishu list_members request failed: {error}"),
                })?;

            let (page_members, page_token) = parse_member_list_response(response).await?;
            members.extend(page_members);

            match page_token {
                Some(token) => next_page_token = Some(token),
                None => return Ok(members),
            }
        }
    }

    pub async fn get_chat_info(&self, chat_id: &str) -> ChannelResult<FeishuChatInfo> {
        let token = self.fetch_tenant_access_token().await?;
        let response = self
            .client
            .get(format!(
                "{}/open-apis/im/v1/chats/{chat_id}",
                self.config.base_url()
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("feishu get_chat_info request failed: {error}"),
            })?;

        parse_chat_info_response(response).await
    }

    async fn fetch_tenant_access_token(&self) -> ChannelResult<String> {
        let app_secret =
            std::env::var(&self.config.app_secret_env).map_err(|_| ChannelError::Config {
                message: format!(
                    "environment variable `{}` is not set",
                    self.config.app_secret_env
                ),
            })?;

        let response = self
            .client
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                self.config.base_url()
            ))
            .json(&TenantTokenRequest {
                app_id: &self.config.app_id,
                app_secret: &app_secret,
            })
            .send()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("tenant access token request failed: {error}"),
            })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("failed to read tenant access token response: {error}"),
            })?;
        if !status.is_success() {
            return Err(ChannelError::Delivery {
                message: format!(
                    "tenant access token request failed with status {status}: {}",
                    summarize_response_body(&body)
                ),
            });
        }

        let payload = serde_json::from_str::<TenantTokenResponse>(&body).map_err(|error| {
            ChannelError::Transport {
                message: format!("invalid tenant access token response: {error}"),
            }
        })?;
        if payload.code != 0 {
            return Err(ChannelError::Delivery {
                message: format!(
                    "tenant access token request failed: {}",
                    payload.msg.unwrap_or_else(|| "unknown error".to_string())
                ),
            });
        }

        payload
            .tenant_access_token
            .ok_or_else(|| ChannelError::Delivery {
                message: "tenant access token response missing token".to_string(),
            })
    }

    async fn send_message_content(
        &self,
        token: &str,
        conversation_id: &str,
        reply_to_message_id: Option<&str>,
        msg_type: &str,
        content: String,
        action: &str,
    ) -> ChannelResult<Option<String>> {
        let response = if let Some(reply_to_message_id) = reply_to_message_id {
            self.client
                .post(format!(
                    "{}/open-apis/im/v1/messages/{reply_to_message_id}/reply",
                    self.config.base_url()
                ))
                .bearer_auth(token)
                .json(&ReplyMessageRequest { msg_type, content })
                .send()
                .await
                .map_err(|error| ChannelError::Transport {
                    message: format!("feishu {action} reply request failed: {error}"),
                })?
        } else {
            self.client
                .post(format!(
                    "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
                    self.config.base_url()
                ))
                .bearer_auth(token)
                .json(&SendMessageRequest {
                    receive_id: conversation_id,
                    msg_type,
                    content,
                })
                .send()
                .await
                .map_err(|error| ChannelError::Transport {
                    message: format!("feishu {action} send request failed: {error}"),
                })?
        };

        parse_send_message_response(response, action).await
    }
}

async fn parse_send_message_response(
    response: reqwest::Response,
    action: &str,
) -> ChannelResult<Option<String>> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to read feishu {action} response: {error}"),
        })?;

    parse_send_message_payload(status, &body, action)
}

fn parse_send_message_payload(
    status: StatusCode,
    body: &str,
    action: &str,
) -> ChannelResult<Option<String>> {
    if !status.is_success() {
        return Err(ChannelError::Delivery {
            message: format!(
                "feishu {action} failed with status {status}: {}",
                summarize_response_body(body)
            ),
        });
    }

    let payload = serde_json::from_str::<SendMessageResponse>(body).map_err(|error| {
        ChannelError::Transport {
            message: format!("invalid feishu {action} response: {error}"),
        }
    })?;
    if payload.code != 0 {
        return Err(ChannelError::Delivery {
            message: format!(
                "feishu {action} failed: {}",
                payload.msg.unwrap_or_else(|| "unknown error".to_string())
            ),
        });
    }

    Ok(payload.data.and_then(|data| data.message_id))
}

async fn ensure_api_success_response(
    response: reqwest::Response,
    action: &str,
) -> ChannelResult<()> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to read {action} response: {error}"),
        })?;
    ensure_api_success_payload(status, &body, action)
}

fn ensure_api_success_payload(status: StatusCode, body: &str, action: &str) -> ChannelResult<()> {
    if !status.is_success() {
        return Err(ChannelError::Delivery {
            message: format!(
                "{action} failed with status {status}: {}",
                summarize_response_body(body)
            ),
        });
    }

    let payload =
        serde_json::from_str::<ApiResponse>(body).map_err(|error| ChannelError::Transport {
            message: format!("invalid {action} response: {error}"),
        })?;
    if payload.code != 0 {
        return Err(ChannelError::Delivery {
            message: format!(
                "{action} failed: {}",
                payload.msg.unwrap_or_else(|| "unknown error".to_string())
            ),
        });
    }
    Ok(())
}

async fn parse_member_list_response(
    response: reqwest::Response,
) -> ChannelResult<(Vec<FeishuChatMember>, Option<String>)> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to read feishu list_members response: {error}"),
        })?;

    parse_member_list_payload(status, &body)
}

fn parse_member_list_payload(
    status: StatusCode,
    body: &str,
) -> ChannelResult<(Vec<FeishuChatMember>, Option<String>)> {
    let payload = serde_json::from_str::<Value>(body).map_err(|error| ChannelError::Transport {
        message: format!("invalid feishu list_members response: {error}"),
    })?;

    if !status.is_success() {
        return Err(ChannelError::Delivery {
            message: format!(
                "feishu list_members failed with status {status}: {}",
                summarize_response_body(body)
            ),
        });
    }

    if payload.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
        let message = payload
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(ChannelError::Delivery {
            message: format!("feishu list_members failed: {message}"),
        });
    }

    let data = payload.get("data").ok_or_else(|| ChannelError::Transport {
        message: "feishu list_members response missing data".to_string(),
    })?;

    let items =
        data.get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| ChannelError::Transport {
                message: "feishu list_members response missing data.items".to_string(),
            })?;

    let members = items
        .iter()
        .filter_map(|item| {
            let id = extract_member_id(item)?;
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    item.get("member_name")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                });
            Some(FeishuChatMember { id, name })
        })
        .collect::<Vec<_>>();

    let has_more = data
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next_page_token = if has_more {
        Some(
            data.get("page_token")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ChannelError::Transport {
                    message: "feishu list_members response missing page_token".to_string(),
                })?
                .to_string(),
        )
    } else {
        None
    };

    Ok((members, next_page_token))
}

async fn parse_chat_info_response(response: reqwest::Response) -> ChannelResult<FeishuChatInfo> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to read feishu get_chat_info response: {error}"),
        })?;

    parse_chat_info_payload(status, &body)
}

fn parse_chat_info_payload(status: StatusCode, body: &str) -> ChannelResult<FeishuChatInfo> {
    if !status.is_success() {
        return Err(ChannelError::Delivery {
            message: format!(
                "feishu get_chat_info failed with status {status}: {}",
                summarize_response_body(body)
            ),
        });
    }

    let envelope = serde_json::from_str::<ChatInfoEnvelope>(body).map_err(|error| {
        ChannelError::Transport {
            message: format!("invalid feishu get_chat_info response: {error}"),
        }
    })?;
    if envelope.code != 0 {
        return Err(ChannelError::Delivery {
            message: format!(
                "feishu get_chat_info failed: {}",
                envelope.msg.unwrap_or_else(|| "unknown error".to_string())
            ),
        });
    }

    let data = envelope.data.ok_or_else(|| ChannelError::Transport {
        message: "feishu get_chat_info response missing data".to_string(),
    })?;

    Ok(FeishuChatInfo {
        owner_id: data.owner_id,
        chat_type: data.chat_type,
        user_manager_ids: data.user_manager_id_list,
        bot_manager_ids: data.bot_manager_id_list,
    })
}

pub fn build_progress_card_content(progress: &ChannelProgressUpdate) -> ChannelResult<String> {
    let template = match progress.state {
        ChannelProgressState::Running => "blue",
        ChannelProgressState::Completed => "green",
        ChannelProgressState::Failed => "red",
    };

    let mut elements = Vec::new();
    elements.push(serde_json::json!({
        "tag": "div",
        "text": {
            "tag": "lark_md",
            "content": escape_lark_md(&progress.summary),
        }
    }));

    for ChannelProgressSection { heading, lines } in &progress.sections {
        if lines.is_empty() {
            continue;
        }

        elements.push(serde_json::json!({ "tag": "hr" }));
        let body = lines
            .iter()
            .map(|line| format!("- {}", escape_lark_md(line)))
            .collect::<Vec<_>>()
            .join("\n");
        elements.push(serde_json::json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": format!("**{}**\n{}", escape_lark_md(heading), body),
            }
        }));
    }

    serde_json::to_string(&serde_json::json!({
        "config": {
            "wide_screen_mode": true,
            "update_multi": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": progress.title,
            },
            "template": template
        },
        "elements": elements
    }))
    .map_err(|error| ChannelError::Delivery {
        message: format!("failed to serialize feishu progress card: {error}"),
    })
}

fn escape_lark_md(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn summarize_response_body(body: &str) -> String {
    let summary = body.trim().replace('\n', " ");
    if summary.is_empty() {
        return "empty response body".to_string();
    }
    if summary.len() <= 256 {
        summary
    } else {
        format!("{}...", &summary[..256])
    }
}

fn extract_member_id(item: &Value) -> Option<String> {
    item.get("member_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            item.get("open_id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            item.get("user_id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::{build_progress_card_content, parse_chat_info_payload, parse_member_list_payload};
    use crate::channels::{ChannelProgressSection, ChannelProgressState, ChannelProgressUpdate};
    use reqwest::StatusCode;
    use serde_json::Value;

    #[test]
    fn builds_progress_card_content_with_sections() {
        let progress = ChannelProgressUpdate {
            title: "小欧正在处理".to_string(),
            summary: "解析群聊需求".to_string(),
            state: ChannelProgressState::Running,
            sections: vec![ChannelProgressSection {
                heading: "状态".to_string(),
                lines: vec!["已识别为群聊任务".to_string(), "开始规划".to_string()],
            }],
        };

        let card = build_progress_card_content(&progress).expect("card content should serialize");
        let payload = serde_json::from_str::<Value>(&card).expect("card should be valid json");

        assert_eq!(payload["header"]["template"], "blue");
        assert_eq!(payload["header"]["title"]["content"], "小欧正在处理");
        assert_eq!(payload["elements"][0]["text"]["content"], "解析群聊需求");
        assert_eq!(
            payload["elements"][2]["text"]["content"],
            "**状态**\n- 已识别为群聊任务\n- 开始规划"
        );
    }

    #[test]
    fn parses_member_list_page_with_pagination() {
        let payload = r#"{
          "code": 0,
          "msg": "success",
          "data": {
            "items": [
              { "member_id": "ou_a", "name": "Alice" },
              { "open_id": "ou_b", "member_name": "Bob" }
            ],
            "has_more": true,
            "page_token": "next-page"
          }
        }"#;

        let (members, next_page) =
            parse_member_list_payload(StatusCode::OK, payload).expect("member list should parse");

        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id, "ou_a");
        assert_eq!(members[0].name.as_deref(), Some("Alice"));
        assert_eq!(members[1].id, "ou_b");
        assert_eq!(members[1].name.as_deref(), Some("Bob"));
        assert_eq!(next_page.as_deref(), Some("next-page"));
    }

    #[test]
    fn parses_chat_info_payload() {
        let payload = r#"{
          "code": 0,
          "msg": "success",
          "data": {
            "owner_id": "ou_owner",
            "chat_type": "group",
            "user_manager_id_list": ["ou_mgr"],
            "bot_manager_id_list": ["cli_bot"]
          }
        }"#;

        let info =
            parse_chat_info_payload(StatusCode::OK, payload).expect("chat info should parse");

        assert_eq!(info.owner_id.as_deref(), Some("ou_owner"));
        assert_eq!(info.chat_type.as_deref(), Some("group"));
        assert_eq!(info.user_manager_ids, vec!["ou_mgr".to_string()]);
        assert_eq!(info.bot_manager_ids, vec!["cli_bot".to_string()]);
    }
}
