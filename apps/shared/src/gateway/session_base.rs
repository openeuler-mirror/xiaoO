//! Session-open / close / cancel / heartbeat / detach / interaction wire
//! request types.
//!
//! The request struct definitions live in [`protocol::wire`]; this
//! module re-exports them under the `xiaoo_shared::gateway` path so existing
//! import paths keep working. The fork / input / submit-receipt types below
//! are not part of the wire contract and stay defined here, alongside the
//! [`channel_session_id`] helper.

use crate::backend::BackendForkResult;
use crate::gateway::{AppTurnRequest, SessionRecord};
use agent_types::interaction::InteractionResponse;
use serde::{Deserialize, Serialize};

pub use protocol::wire::{
    RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, SessionCancelRequest, SessionCloseRequest,
    SessionDetachRequest, SessionHeartbeatRequest, SessionInteractionRequest, SessionOpenRequest,
};

/// Derive the session id used for a channel-scoped conversation.
///
/// Falls back to the channel name when no instance id is given, so each
/// channel defaults to a single conversation scope unless the caller
/// distinguishes instances.
pub fn channel_session_id(
    channel: &str,
    channel_instance_id: Option<&str>,
    conversation_id: &str,
) -> String {
    let scope = channel_instance_id.unwrap_or(channel);
    format!("{scope}:{conversation_id}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct SessionForkRequest {
    pub parent_session_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub sender_id: Option<String>,
    #[serde(default)]
    pub snapshot_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) struct SessionForkResult {
    pub parent: SessionRecord,
    pub child: SessionRecord,
    pub backend_fork: BackendForkResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionInput {
    Turn {
        request: AppTurnRequest,
    },
    Interaction {
        response: InteractionResponse,
    },
    InputChunk {
        stream_id: String,
        seq: u32,
        content: String,
        is_final: bool,
    },
    CancelActiveTurn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionInputKind {
    Turn,
    Interaction,
    InputChunk,
    CancelActiveTurn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSubmitReceipt {
    pub session_id: String,
    pub accepted_kind: SessionInputKind,
}
