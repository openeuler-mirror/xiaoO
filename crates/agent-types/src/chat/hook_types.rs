use crate::llm::ChatMessage;
use serde::{Deserialize, Serialize};

/// Lightweight model reference carried by chat-level hook inputs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

/// Carries slash-command metadata through the turn pipeline so the
/// `*.Chat.command.before` hooker can fire with `{ command, arguments }`.
/// Populated by the TUI when the user input originated from
/// `~/.xiaoo/commands/<name>.md`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandContext {
    pub command: String,
    pub arguments: String,
}

/// Input for `*.Chat.system.transform`. Fires in `build_messages` after the
/// prompt builder produces the ordered `system: Vec<String>` parts and before
/// they are joined into the single system message. `current_system` is the
/// parts array the plugin may rewrite.
#[derive(Clone, Debug, Default)]
pub struct ChatSystemTransformInput {
    pub session_id: Option<String>,
    pub model: ModelRef,
    pub current_system: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum ChatSystemTransformResult {
    Allow,
    Transform { system: Vec<String> },
}

/// Input for `*.Chat.message.received`. Fires when a user message is
/// constructed but before it is persisted.
#[derive(Clone, Debug)]
pub struct ChatMessageHookInput {
    pub session_id: String,
    pub agent: Option<String>,
    pub model: Option<ModelRef>,
    pub message_id: Option<String>,
    pub message: ChatMessage,
    /// Number of messages already in the conversation history at the
    /// moment this hook fires. The current user message is NOT yet
    /// persisted, so this is the count of prior messages. Plugins use
    /// `prior_message_count <= 1` to detect the "first effective user
    /// input" of a session (handles retry/recovery where only a user
    /// message was persisted without an assistant reply).
    pub prior_message_count: usize,
}

#[derive(Clone, Debug)]
pub enum ChatMessageHookResult {
    Accept,
    Transform { message: ChatMessage },
}

/// Input for `*.Chat.command.before`. Fires after a slash command template
/// is expanded and before the body is submitted as a user turn. `body` is
/// the expanded body the plugin may rewrite.
#[derive(Clone, Debug, Default)]
pub struct CommandExecuteBeforeInput {
    pub command: String,
    pub session_id: String,
    pub arguments: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub enum CommandExecuteBeforeResult {
    Allow,
    Transform { body: String },
    Deny { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ChatHookError {
    /// A plugin command hooker failed at the process/IO/JSON layer — spawn
    /// failure, stdin write, non-zero exit, invalid JSON, missing field, or
    /// an unsupported result/action. Carries the human-readable detail
    /// surfaced to logs and trace spans.
    #[error("{message}")]
    Plugin { message: String },
}
