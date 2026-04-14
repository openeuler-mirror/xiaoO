use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatewayEntryKind {
    Channel,
    Tui,
    HttpApi,
    ScheduledJob,
    Cli,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GatewayEntryContext {
    pub kind: Option<GatewayEntryKind>,
    pub instance_id: Option<String>,
    pub runtime_profile_id: Option<String>,
    #[serde(default)]
    pub build_tags: Vec<String>,
}

impl GatewayEntryContext {
    pub fn channel(instance_id: Option<String>) -> Self {
        Self {
            kind: Some(GatewayEntryKind::Channel),
            instance_id,
            runtime_profile_id: None,
            build_tags: Vec::new(),
        }
    }

    pub fn tui(instance_id: Option<String>) -> Self {
        Self {
            kind: Some(GatewayEntryKind::Tui),
            instance_id,
            runtime_profile_id: None,
            build_tags: Vec::new(),
        }
    }

    pub fn cli() -> Self {
        Self {
            kind: Some(GatewayEntryKind::Cli),
            instance_id: None,
            runtime_profile_id: None,
            build_tags: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnMention {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppTurnRequest {
    pub session_id: String,
    #[serde(default)]
    pub entry: GatewayEntryContext,
    pub channel: Option<String>,
    pub message_id: Option<String>,
    pub conversation_id: String,
    pub sender_id: String,
    pub text: String,
    pub channel_instance_id: Option<String>,
    #[serde(default)]
    pub channel_identity_prompt: Option<String>,
    pub reply_to_message_id: Option<String>,
    pub root_message_id: Option<String>,
    pub mentions: Vec<TurnMention>,
}
