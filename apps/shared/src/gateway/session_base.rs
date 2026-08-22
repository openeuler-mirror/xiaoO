//! Session-open / close / cancel / heartbeat / detach / interaction wire
//! request types.
//!
//! The request struct definitions live in [`xiaoo_protocol::wire`]; this
//! module re-exports them under the `xiaoo_shared::gateway` path so existing
//! import paths keep working. Only [`channel_session_id`] is defined here —
//! it is a pure helper with no wire-contract surface.

pub use xiaoo_protocol::wire::{
    RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, SessionCancelRequest, SessionCloseRequest,
    SessionDetachRequest, SessionHeartbeatRequest, SessionInteractionRequest,
    SessionOpenRequest,
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
