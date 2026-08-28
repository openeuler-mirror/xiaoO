//! Wire request types for the daemon HTTP/SSE protocol.
//!
//! This module owns the client-facing request types that flow over the
//! `/api/v1/runtimes/*` HTTP surface: [`SessionOpenRequest`] (aliased
//! [`RuntimeOpenRequest`]), [`AppTurnRequest`] (aliased
//! [`RuntimeTurnRequest`]), and the close/cancel/heartbeat/detach/interaction
//! request structs.  Their serde representation is a wire contract: any field
//! change must stay byte-for-byte compatible with the daemon's serialization,
//! and changes are treated as protocol changes coordinated with the daemon
//! repository.
//!
//! The supporting entry-context and LLM-config types
//! ([`GatewayEntryContext`] / [`GatewayEntryKind`] / [`LlmRuntimeConfig`] /
//! [`TurnMention`]) live here because they are fields of the request structs
//! and must be constructible from any client that builds a wire request.
//! `xiaoo_shared::gateway` re-exports all of these to preserve existing import
//! paths; the request types carry no service-layer state, only serialized
//! fields, so defining them in the protocol crate keeps the boundary clean.

use agent_types::chat::CommandContext;
use agent_types::interaction::InteractionResponse;
use agent_types::ReasoningEffort;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Originating entry surface for a runtime request (channel, TUI, CLI, …).
///
/// Carried on open/turn requests so the daemon can branch on entry kind
/// (e.g. noop tool registry for TUI/CLI vs full registry for channel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatewayEntryKind {
    /// Originated from a chat channel integration.
    Channel,
    /// Originated from the terminal UI.
    Tui,
    /// Originated from the HTTP API.
    HttpApi,
    /// Originated from a scheduled/cron job.
    ScheduledJob,
    /// Originated from the CLI.
    Cli,
    /// Originated from an MCP caller.
    Mcp,
}

/// Context describing how a runtime request entered the gateway.
///
/// Default-constructs to an "unknown entry" (all fields `None`/empty) which
/// the daemon treats as an anonymous caller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GatewayEntryContext {
    /// Entry surface kind. `None` means the caller did not state one.
    pub kind: Option<GatewayEntryKind>,
    /// Entry-instance identifier (e.g. channel id), when applicable.
    pub instance_id: Option<String>,
    /// Selected runtime profile id, when the caller pinned one.
    pub runtime_profile_id: Option<String>,
    /// Build tags that the daemon uses to select a runtime variant.
    #[serde(default)]
    pub build_tags: Vec<String>,
}

impl GatewayEntryContext {
    /// Construct a channel entry context with an optional instance id.
    pub fn channel(instance_id: Option<String>) -> Self {
        Self {
            kind: Some(GatewayEntryKind::Channel),
            instance_id,
            runtime_profile_id: None,
            build_tags: Vec::new(),
        }
    }

    /// Construct a TUI entry context with an optional instance id.
    pub fn tui(instance_id: Option<String>) -> Self {
        Self {
            kind: Some(GatewayEntryKind::Tui),
            instance_id,
            runtime_profile_id: None,
            build_tags: Vec::new(),
        }
    }

    /// Construct a CLI entry context (no instance id).
    pub fn cli() -> Self {
        Self {
            kind: Some(GatewayEntryKind::Cli),
            instance_id: None,
            runtime_profile_id: None,
            build_tags: Vec::new(),
        }
    }
}

/// Runtime-scoped LLM configuration that a client may pin on a request.
///
/// All fields optional and default-emptied; absent fields fall back to the
/// daemon's resolved provider/model/api-key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmRuntimeConfig {
    /// Provider id (e.g. `ollama`, `openai`), overriding the daemon default.
    #[serde(default)]
    pub provider: Option<String>,
    /// Model id, overriding the daemon default.
    #[serde(default)]
    pub model: Option<String>,
    /// API base URL, overriding the daemon default.
    #[serde(default)]
    pub api_base: Option<String>,
    /// Environment variable name holding the api key (resolved daemon-side).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Inline api key (rarely used; prefer `api_key_env`).
    #[serde(default)]
    pub api_key: Option<String>,
}

/// A user/channel mention carried on a turn request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnMention {
    /// Stable mention id (e.g. `@user-handle`).
    pub id: String,
    /// Optional display name for rendering.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A turn request body sent to `/api/v1/runtimes/turn` (and the open/turn
/// combined path).
///
/// `runtime_id` is the on-the-wire name; `session_id` is accepted as a legacy
/// alias for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppTurnRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Entry context describing how the turn was initiated.
    #[serde(default)]
    pub entry: GatewayEntryContext,
    /// Channel name, when the turn originates from a channel integration.
    #[serde(default)]
    pub channel: Option<String>,
    /// Caller-assigned message id, when dedup/ordering matters.
    #[serde(default)]
    pub message_id: Option<String>,
    /// Conversation id the turn belongs to.
    pub conversation_id: String,
    /// Sender id (user or principal) issuing the turn.
    pub sender_id: String,
    /// Turn text payload.
    pub text: String,
    /// Channel instance id (for multi-instance channels).
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    /// Optional prompt injected for channel identity context.
    #[serde(default)]
    pub channel_identity_prompt: Option<String>,
    /// Message id this turn replies to, for threading.
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    /// Root message id of the thread this turn belongs to.
    #[serde(default)]
    pub root_message_id: Option<String>,
    /// Mentions carried by the turn text.
    pub mentions: Vec<TurnMention>,
    /// Reasoning effort hint for the model.
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    /// Runtime-scoped LLM config override.
    #[serde(default)]
    pub llm: Option<LlmRuntimeConfig>,
    /// Absolute path on the daemon host to snapshot into a newly-created E2B runtime.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Ordered daemon-host skill search roots. Each root contains skill directories.
    #[serde(default)]
    pub skills: Option<Vec<PathBuf>>,
    /// When the turn originates from a slash command
    /// (`~/.xiaoo/commands/<name>.md`), carries the command name and raw
    /// arguments so the `*.Chat.command.before` hooker can fire with full
    /// command metadata. `None` for free-form user input.
    #[serde(default)]
    pub command_context: Option<CommandContext>,
    /// Cross-turn `send_prompt` chain depth. `0` for normal user-typed /
    /// HTTP API turns (resets the chain). When the TUI executes a
    /// `SendPrompt` hook action, it relays the daemon-stamped
    /// `action.chain_depth` here so the daemon tracks the resulting turn's
    /// depth and can enforce the cap (`[hooker].max_prompt_chain_depth`,
    /// default 128). Set by the host; plugins cannot influence it directly.
    #[serde(default)]
    pub chain_depth: usize,
    /// Process identifier propagated from `SessionOpenRequest.client_id`.
    /// The `SessionActor` uses it to fail-fast queued turns whose holder
    /// changed after a takeover. `None` for legacy / anonymous callers.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// Alias for [`AppTurnRequest`] under the runtime-protocol naming.
pub type RuntimeTurnRequest = AppTurnRequest;

/// A session-open request body sent to `/api/v1/runtimes/open`.
///
/// `runtime_id` is the on-the-wire name; `session_id` is accepted as a legacy
/// alias for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionOpenRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Conversation id to open the runtime under.
    pub conversation_id: String,
    /// Sender id (user or principal) opening the runtime.
    pub sender_id: String,
    /// Entry context describing how the open was initiated.
    #[serde(default)]
    pub entry: GatewayEntryContext,
    /// Channel name, when opening on behalf of a channel integration.
    #[serde(default)]
    pub channel: Option<String>,
    /// Channel instance id (for multi-instance channels).
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    /// Runtime-scoped LLM config override.
    #[serde(default)]
    pub llm: Option<LlmRuntimeConfig>,
    /// Absolute path on the daemon host to snapshot into an E2B runtime.
    /// Other backends ignore this field and retain their configured workspace.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Ordered daemon-host search roots whose immediate child directories are skills.
    #[serde(default)]
    pub skills: Option<Vec<PathBuf>>,
    /// Process identifier used by the daemon's attach-lease table to enforce
    /// single-writer per session. `None` for legacy / anonymous callers
    /// (bypass during rollout).
    #[serde(default)]
    pub client_id: Option<String>,
    /// Display-only client PID / hostname, surfaced to the TUI on "who holds
    /// the lease?" queries. Not authoritative.
    #[serde(default)]
    pub client_pid: Option<u32>,
    /// Display-only client hostname companion to [`Self::client_pid`].
    #[serde(default)]
    pub client_hostname: Option<String>,
}

impl SessionOpenRequest {
    /// Convert an open request into a turn request carrying the same
    /// runtime/entry/llm/workspace/skills/client identity, with `text` as the
    /// turn payload and chain depth reset to 0.
    pub fn into_turn_request(self, text: String) -> AppTurnRequest {
        AppTurnRequest {
            session_id: self.session_id,
            entry: self.entry,
            channel: self.channel,
            message_id: None,
            conversation_id: self.conversation_id,
            sender_id: self.sender_id,
            text,
            channel_instance_id: self.channel_instance_id,
            channel_identity_prompt: None,
            reply_to_message_id: None,
            root_message_id: None,
            mentions: Vec::new(),
            reasoning_effort: Default::default(),
            llm: self.llm,
            workspace: self.workspace,
            skills: self.skills,
            command_context: None,
            chain_depth: 0,
            client_id: self.client_id,
        }
    }
}

/// A session-close request body sent to `/api/v1/runtimes/close`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCloseRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Process identifier of the closer, for lease-table bookkeeping.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// A session-cancel request body sent to `/api/v1/runtimes/cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCancelRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Process identifier of the canceller, for lease-table bookkeeping.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// A session-heartbeat request body sent to `/api/v1/runtimes/heartbeat`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionHeartbeatRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Process identifier of the heartbeating client.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Display-only client PID / hostname, stamped onto the lease on
    /// auto-re-acquire (daemon restart wiped the table) so holder identity
    /// survives.
    #[serde(default)]
    pub client_pid: Option<u32>,
    /// Display-only client hostname companion to [`Self::client_pid`].
    #[serde(default)]
    pub client_hostname: Option<String>,
}

/// A session-detach request body sent to `/api/v1/runtimes/detach`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionDetachRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Process identifier of the detaching client.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// A session-interaction request body carrying an interaction response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInteractionRequest {
    /// Runtime (session) id, serialized as `runtime_id`.
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    /// Interaction response being delivered to a pending interaction.
    pub response: InteractionResponse,
    /// Process identifier of the responding client.
    #[serde(default)]
    pub client_id: Option<String>,
}

/// Alias for [`SessionOpenRequest`] under the runtime-protocol naming.
pub type RuntimeOpenRequest = SessionOpenRequest;
/// Alias for [`SessionCloseRequest`] under the runtime-protocol naming.
pub type RuntimeCloseRequest = SessionCloseRequest;
/// Alias for [`SessionCancelRequest`] under the runtime-protocol naming.
pub type RuntimeCancelRequest = SessionCancelRequest;
/// Alias for [`SessionInteractionRequest`] under the runtime-protocol naming.
pub type RuntimeInteractionRequest = SessionInteractionRequest;
/// Alias for [`SessionHeartbeatRequest`] under the runtime-protocol naming.
pub type RuntimeHeartbeatRequest = SessionHeartbeatRequest;
/// Alias for [`SessionDetachRequest`] under the runtime-protocol naming.
pub type RuntimeDetachRequest = SessionDetachRequest;

#[cfg(test)]
#[path = "wire/tests.rs"]
mod tests;
