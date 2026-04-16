use crate::channels::ChannelAdapter;
use crate::gateway::pending_interaction::{PendingInteraction, PendingInteractionStore};
use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// InteractionHandle implementation for channel-based sessions (Feishu, etc.).
pub struct ChannelInteractionHandle {
    session_id: String,
    conversation_id: String,
    reply_to_message_id: Option<String>,
    pending_store: Arc<PendingInteractionStore>,
    adapter: Arc<dyn ChannelAdapter>,
    timeout: Duration,
    timeout_secs: u64,
}

impl ChannelInteractionHandle {
    pub fn new(
        timeout_secs: u64,
        session_id: String,
        conversation_id: String,
        reply_to_message_id: Option<String>,
        pending_store: Arc<PendingInteractionStore>,
        adapter: Arc<dyn ChannelAdapter>,
    ) -> Self {
        // Round up to whole minutes, minimum 1 minute.
        let timeout_minutes = ((timeout_secs + 59) / 60).max(1);
        let actual_timeout_secs = timeout_minutes * 60;
        Self {
            session_id,
            conversation_id,
            reply_to_message_id,
            pending_store,
            adapter,
            timeout: Duration::from_secs(actual_timeout_secs),
            timeout_secs: actual_timeout_secs,
        }
    }

    fn timeout_minutes(&self) -> u64 {
        self.timeout_secs / 60
    }

    fn format_prompt(&self, request: &InteractionRequest) -> String {
        let mins = self.timeout_minutes();
        let timeout_hint = format!("{} \u{5206}\u{949f}\u{5185}\u{672a}\u{56de}\u{590d}\u{5c06}\u{81ea}\u{52a8}\u{53d6}\u{6d88}", mins);
        match request {
            InteractionRequest::Confirm { prompt, .. } => {
                format!(
                    "{}\n\n\u{ff08}\u{8bf7}\u{56de}\u{590d}\u{201c}\u{662f}\u{201d}\u{6216}\u{201c}\u{5426}\u{201d}\u{ff0c}{}\u{ff09}",
                    prompt, timeout_hint
                )
            }
            InteractionRequest::TextInput { prompt, .. } => {
                format!(
                    "{}\n\n\u{ff08}\u{8bf7}\u{76f4}\u{63a5}\u{56de}\u{590d}\u{ff0c}{}\u{ff09}",
                    prompt, timeout_hint
                )
            }
            InteractionRequest::Choice {
                prompt,
                options,
                allow_custom_input,
                ..
            } => {
                let mut text = format!("{}\n", prompt);
                for (i, option) in options.iter().enumerate() {
                    text.push_str(&format!("\n{}. {}", i + 1, option));
                }
                if *allow_custom_input {
                    text.push_str(&format!(
                        "\n\n\u{ff08}\u{8bf7}\u{56de}\u{590d}\u{9009}\u{9879}\u{7f16}\u{53f7}\u{6216}\u{8f93}\u{5165}\u{81ea}\u{5b9a}\u{4e49}\u{5185}\u{5bb9}\u{ff0c}{}\u{ff09}",
                        timeout_hint
                    ));
                } else {
                    text.push_str(&format!(
                        "\n\n\u{ff08}\u{8bf7}\u{56de}\u{590d}\u{9009}\u{9879}\u{7f16}\u{53f7}\u{ff0c}{}\u{ff09}",
                        timeout_hint
                    ));
                }
                text
            }
        }
    }

    fn timeout_sentinel(&self) -> String {
        let mins = self.timeout_minutes();
        format!(
            "[INTERACTION_TIMEOUT] The user did not reply within {} minutes. \
             Do NOT continue this task. Tell the user the task has been \
             cancelled due to timeout and they can start a new request.",
            mins
        )
    }

    fn timeout_notice(&self) -> String {
        let mins = self.timeout_minutes();
        format!(
            "\u{23f0} \u{5df2}\u{8d85}\u{65f6}\u{ff0c}\u{60a8}\u{672a}\u{5728} {} \u{5206}\u{949f}\u{5185}\u{56de}\u{590d}\u{3002}\u{5f53}\u{524d}\u{4efb}\u{52a1}\u{5df2}\u{53d6}\u{6d88}\u{ff0c}\u{5982}\u{9700}\u{7ee7}\u{7eed}\u{8bf7}\u{91cd}\u{65b0}\u{53d1}\u{8d77}\u{8bf7}\u{6c42}\u{3002}",
            mins
        )
    }
}

#[async_trait]
impl InteractionHandle for ChannelInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let prompt_text = self.format_prompt(request);

        if let Err(error) = self
            .adapter
            .send_text(
                &self.conversation_id,
                &prompt_text,
                self.reply_to_message_id.as_deref(),
            )
            .await
        {
            tracing::warn!("channel interaction: failed to send prompt: {error}");
            return self.timeout_response(request);
        }

        let (tx, rx) = oneshot::channel();
        self.pending_store
            .register(
                &self.session_id,
                PendingInteraction {
                    request: request.clone(),
                    response_tx: tx,
                    created_at: Instant::now(),
                },
            )
            .await;

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                tracing::warn!("channel interaction: response channel closed");
                self.timeout_response(request)
            }
            Err(_) => {
                self.pending_store.take(&self.session_id).await;
                tracing::warn!(
                    "channel interaction: timed out after {}s",
                    self.timeout_secs
                );
                let _ = self
                    .adapter
                    .send_text(
                        &self.conversation_id,
                        &self.timeout_notice(),
                        self.reply_to_message_id.as_deref(),
                    )
                    .await;
                self.timeout_response(request)
            }
        }
    }
}

impl ChannelInteractionHandle {
    fn timeout_response(&self, request: &InteractionRequest) -> InteractionResponse {
        let sentinel = self.timeout_sentinel();
        match request {
            InteractionRequest::Confirm { .. } => {
                InteractionResponse::Confirmed { allowed: false }
            }
            InteractionRequest::TextInput { .. } => InteractionResponse::Text {
                value: Some(sentinel),
            },
            InteractionRequest::Choice { .. } => InteractionResponse::Choice {
                value: Some(sentinel),
            },
        }
    }
}

/// Map the raw user text reply back to a typed InteractionResponse.
pub fn resolve_interaction_from_text(
    text: &str,
    request: &InteractionRequest,
) -> InteractionResponse {
    let trimmed = text.trim();
    match request {
        InteractionRequest::Confirm { .. } => {
            let yes = matches!(
                trimmed.to_lowercase().as_str(),
                "yes" | "y" | "ok" | "1" | "true"
                    | "\u{662f}"          // 是
                    | "\u{786e}\u{8ba4}"  // 确认
                    | "\u{597d}"          // 好
                    | "\u{597d}\u{7684}"  // 好的
                    | "\u{884c}"          // 行
            );
            InteractionResponse::Confirmed { allowed: yes }
        }
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: Some(trimmed.to_string()),
        },
        InteractionRequest::Choice { options, .. } => {
            if let Ok(index) = trimmed.parse::<usize>() {
                if index >= 1 && index <= options.len() {
                    return InteractionResponse::Choice {
                        value: Some(options[index - 1].clone()),
                    };
                }
            }
            InteractionResponse::Choice {
                value: Some(trimmed.to_string()),
            }
        }
    }
}
