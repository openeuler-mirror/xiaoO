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

/// Terminal kind of a completed (non-error) root turn, mirroring the four
/// `Ok` variants of [`agent_types::outcome::AgentOutcome`]. Carried on
/// [`AppTurnResult`] so downstream consumers (notably the
/// `*.Session.lifecycle.state` plugin hook) can distinguish a normal
/// completion from a soft termination (`max_turns_reached` /
/// `budget_exhausted` / `cancelled`). True errors stay on the `Err` side of
/// `Result<AppTurnResult, _>` and are out of scope for this enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Complete,
    MaxTurnsReached,
    BudgetExhausted,
    Cancelled,
}

impl TurnOutcome {
    /// Snake-case tag used in hook payloads (matches the serde renaming).
    pub fn as_tag(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::MaxTurnsReached => "max_turns_reached",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Cancelled => "cancelled",
        }
    }
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
    pub outcome: TurnOutcome,
    /// Actions requested by `*.Session.lifecycle.state` plugin hookers after
    /// the turn terminated. Collected by `run_turn_inner` (await variant, not
    /// fire-and-forget) and executed daemon-side via `HookActionSink`. Callers
    /// that expose a UI (daemon HTTP/SSE, TUI) forward these to the client so
    /// it can switch session focus / sync titles. CLI and channel callers
    /// currently ignore this field.
    pub hook_actions: Vec<agent_types::hook::HookAction>,
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
    /// When the turn originates from a slash command
    /// (`~/.xiaoo/commands/<name>.md`), carries the command name and raw
    /// arguments so the `*.Chat.command.before` hooker can fire with full
    /// command metadata. `None` for free-form user input.
    #[serde(default)]
    pub command_context: Option<agent_types::chat::CommandContext>,
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
            command_context: None,
        };

        let value = serde_json::to_value(&request).expect("request should serialize");
        assert_eq!(value["runtime_id"], "runtime-1");
        assert!(value.get("session_id").is_none());
    }
}
