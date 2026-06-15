use agent_types::ChatMessage;
use agent_types::ReasoningEffort;
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

#[derive(Debug, Clone)]
pub struct AppTurnResult {
    pub raw_reply: String,
    pub visible_reply: String,
    pub messages: Vec<ChatMessage>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub estimated_input_tokens: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmRuntimeConfig {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnMention {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppTurnRequest {
    #[serde(rename = "runtime_id", alias = "session_id")]
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
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub llm: Option<LlmRuntimeConfig>,
}

pub type RuntimeTurnRequest = AppTurnRequest;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_turn_request_serializes_runtime_id() {
        let request = RuntimeTurnRequest {
            session_id: "runtime-1".to_string(),
            entry: GatewayEntryContext::default(),
            channel: None,
            message_id: None,
            conversation_id: "conv-1".to_string(),
            sender_id: "user-1".to_string(),
            text: "hello".to_string(),
            channel_instance_id: None,
            channel_identity_prompt: None,
            reply_to_message_id: None,
            root_message_id: None,
            mentions: Vec::new(),
            reasoning_effort: ReasoningEffort::default(),
            llm: None,
        };

        let value = serde_json::to_value(&request).expect("request should serialize");
        assert_eq!(value["runtime_id"], "runtime-1");
        assert!(value.get("session_id").is_none());
    }
}
