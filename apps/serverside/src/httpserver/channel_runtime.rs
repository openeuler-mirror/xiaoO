use crate::channels::{
    ChannelAdapter, ChannelError, ChannelOutboundAttachment, ChannelOutboundAttachmentKind,
    ChannelRuntime,
};
use crate::gateway::channel_interaction::{
    resolve_interaction_from_text, ChannelInteractionHandle,
};
use crate::gateway::pending_interaction::PendingInteractionStore;
use crate::gateway::{channel_session_id, ChannelProgressRelayHandle, SessionService};
use crate::httpserver::channel_ingress::{
    build_gateway_channel_message, GatewayChannelIngressError, GatewayChannelMention,
};
use crate::httpserver::{GatewayService, GatewayServiceError};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum ChannelMessageProcessingError {
    #[error(transparent)]
    ChannelIngress(#[from] GatewayChannelIngressError),
    #[error(transparent)]
    Gateway(#[from] GatewayServiceError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
}

#[derive(Clone)]
pub struct ChannelRuntimeProcessor {
    gateway_service: Arc<GatewayService>,
    pending_interactions: Arc<PendingInteractionStore>,
    interaction_timeout_secs: u64,
}

impl ChannelRuntimeProcessor {
    pub fn new(session_service: Arc<dyn SessionService>) -> Self {
        Self::with_timeout(session_service, 600)
    }

    pub fn with_timeout(
        session_service: Arc<dyn SessionService>,
        interaction_timeout_secs: u64,
    ) -> Self {
        Self {
            gateway_service: Arc::new(GatewayService::new(session_service)),
            pending_interactions: Arc::new(PendingInteractionStore::new()),
            interaction_timeout_secs,
        }
    }

    pub async fn process_message(
        &self,
        runtime: ChannelRuntime,
        message: crate::channels::ChannelMessage,
    ) -> Result<(), ChannelMessageProcessingError> {
        let adapter = runtime.adapter.clone();
        let conversation_id = message.conversation_id.clone();
        let reply_to_message_id = message.reply_to_message_id.clone();

        let session_id = channel_session_id(
            &runtime.channel_id,
            Some(&runtime.instance_id),
            &conversation_id,
        );
        if let Some(pending) = self.pending_interactions.take(&session_id).await {
            let response = resolve_interaction_from_text(&message.text, &pending.request);
            let _ = pending.response_tx.send(response);
            return Ok(());
        }

        let progress_relay = runtime.capabilities.supports_progress_updates.then(|| {
            ChannelProgressRelayHandle::new(
                adapter.clone(),
                conversation_id.clone(),
                reply_to_message_id.clone(),
            )
        });
        if let Some(progress_relay) = progress_relay.as_ref() {
            if let Err(error) = progress_relay.mark_received().await {
                warn!("failed to publish initial progress update: {error}");
            }
        }

        let channel_identity_prompt = build_channel_identity_prompt(&runtime, &message).await;
        let event_sink = progress_relay
            .as_ref()
            .map(|relay| Arc::new(relay.clone()) as Arc<dyn LoopEventSink>);
        let mut gateway_message = build_gateway_channel_message(message)?;
        gateway_message.channel_identity_prompt = channel_identity_prompt;

        let interaction_handle: Option<Arc<dyn InteractionHandle>> =
            Some(Arc::new(ChannelInteractionHandle::new(
                self.interaction_timeout_secs,
                session_id,
                conversation_id.clone(),
                reply_to_message_id.clone(),
                self.pending_interactions.clone(),
                adapter.clone(),
            )));

        let turn_response = match self
            .gateway_service
            .handle_channel_message_with_interaction(
                gateway_message,
                event_sink,
                interaction_handle,
                Some(Arc::new(AdapterFileSender {
                    adapter: adapter.clone(),
                    conversation_id: conversation_id.clone(),
                    reply_to_message_id: reply_to_message_id.clone(),
                })),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if let Some(progress_relay) = progress_relay.as_ref() {
                    if let Err(progress_error) =
                        progress_relay.mark_failed(&error.to_string()).await
                    {
                        warn!(
                            "failed to publish gateway failure progress update: {progress_error}"
                        );
                    }
                }
                return Err(error.into());
            }
        };

        if let Err(error) = adapter
            .send_text(
                &conversation_id,
                &turn_response.visible_reply,
                reply_to_message_id.as_deref(),
            )
            .await
        {
            if let Some(progress_relay) = progress_relay.as_ref() {
                if let Err(progress_error) = progress_relay.mark_failed(&error.to_string()).await {
                    warn!("failed to publish delivery failure progress update: {progress_error}");
                }
            }
            return Err(error.into());
        }

        if let Some(progress_relay) = progress_relay.as_ref() {
            if let Err(error) = progress_relay.mark_delivered().await {
                warn!("failed to publish delivered progress update: {error}");
            }
        }

        Ok(())
    }
}

async fn build_channel_identity_prompt(
    runtime: &ChannelRuntime,
    message: &crate::channels::ChannelMessage,
) -> Option<String> {
    let mut participants = Vec::new();
    let mut seen_ids = HashSet::new();

    push_participant(
        &mut participants,
        &mut seen_ids,
        message.sender_id.clone(),
        None,
    );

    for mention in &message.mentions {
        push_participant(
            &mut participants,
            &mut seen_ids,
            mention.id.clone(),
            mention.display_name.clone(),
        );
    }

    if runtime.capabilities.supports_member_listing {
        match runtime.adapter.list_members(&message.conversation_id).await {
            Ok(members) => {
                for member in members {
                    push_participant(
                        &mut participants,
                        &mut seen_ids,
                        member.id,
                        member.display_name,
                    );
                }
            }
            Err(error) => {
                warn!(
                    "failed to load channel member directory: instance={} channel={} conversation={} error={}",
                    runtime.instance_id,
                    runtime.channel_id,
                    message.conversation_id,
                    error
                );
            }
        }
    }

    if participants.is_empty() {
        None
    } else {
        Some(render_participant_directory(&participants))
    }
}

fn push_participant(
    participants: &mut Vec<GatewayChannelMention>,
    seen_ids: &mut HashSet<String>,
    id: String,
    display_name: Option<String>,
) {
    let normalized_id = id.trim();
    if normalized_id.is_empty() {
        return;
    }

    if let Some(existing) = participants
        .iter_mut()
        .find(|participant| participant.id == normalized_id)
    {
        if existing
            .display_name
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            existing.display_name = normalize_display_name(display_name);
        }
        return;
    }

    if seen_ids.insert(normalized_id.to_string()) {
        participants.push(GatewayChannelMention {
            id: normalized_id.to_string(),
            display_name: normalize_display_name(display_name),
        });
    }
}

fn normalize_display_name(display_name: Option<String>) -> Option<String> {
    display_name.and_then(|display_name| {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn render_participant_directory(participants: &[GatewayChannelMention]) -> String {
    let mut rendered = String::from("<participant_directory>");
    for participant in participants {
        let label = participant
            .display_name
            .as_deref()
            .filter(|display_name| !display_name.trim().is_empty())
            .unwrap_or(participant.id.as_str());
        rendered.push_str("\n<person uid=\"");
        rendered.push_str(&escape_xml(participant.id.as_str()));
        rendered.push_str("\">");
        rendered.push_str(&escape_xml(label));
        rendered.push_str("</person>");
    }
    rendered.push_str("\n</participant_directory>");
    rendered
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

struct AdapterFileSender {
    adapter: Arc<dyn ChannelAdapter>,
    conversation_id: String,
    reply_to_message_id: Option<String>,
}

#[async_trait]
impl ChannelFileSender for AdapterFileSender {
    async fn send_file(
        &self,
        file_path: &str,
        label: Option<&str>,
    ) -> Result<Option<String>, String> {
        let attachment = ChannelOutboundAttachment {
            kind: ChannelOutboundAttachmentKind::File,
            path: file_path.to_string(),
            label: label.map(ToString::to_string),
        };
        self.adapter
            .send_attachment(
                &self.conversation_id,
                &attachment,
                self.reply_to_message_id.as_deref(),
            )
            .await
            .map_err(|error| error.to_string())
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
}
