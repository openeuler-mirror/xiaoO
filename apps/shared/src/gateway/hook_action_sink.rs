//! Daemon-side hook action executor.
//!
//! Executes plugin-requested actions against the daemon's own
//! `SessionControlPlane` before forwarding the surviving subset to the TUI.
//! Only the concrete [`DaemonHookActionSink`] type is public; the router is
//! its sole consumer and calls `execute_on_daemon` directly.

use std::sync::Arc;

use xiaoo_api::chat::HookAction;

use crate::gateway::{
    daemon_hook_principal, GatewayEntryContext, SessionControlPlane, SessionOpenRequest,
};

/// Daemon-side hook action executor: runs plugin-requested actions against
/// the daemon's own `SessionControlPlane` before forwarding them to the TUI.
///
/// - `CreateSession` / `SwitchSession`: call `open_session` with the
///   requested `session_id` (idempotent resume), then forward to the TUI
///   so it can switch focus.
/// - `SendPrompt`: call `open_session` to ensure the target session exists
///   (the actual turn is initiated by the TUI, which POSTs to
///   `/api/v1/runtimes/input` for streaming responses). The daemon-stamped
///   `chain_depth` rides along with the forwarded action; the TUI relays it
///   back via `RuntimeTurnRequest.chain_depth` so the daemon can enforce a
///   cross-turn depth limit.
pub struct DaemonHookActionSink {
    control_plane: Arc<dyn SessionControlPlane>,
}

impl DaemonHookActionSink {
    /// Constructs the executor with the given session control plane.
    pub fn new(control_plane: Arc<dyn SessionControlPlane>) -> Self {
        Self { control_plane }
    }

    /// Executes a batch of hook actions on the daemon side, returning the
    /// subset that should be forwarded to the TUI.
    pub async fn execute_on_daemon(&self, actions: Vec<HookAction>) -> Vec<HookAction> {
        let max = agent_contracts::MAX_ACTION_DEPTH;
        let actions = if actions.len() > max {
            let dropped = actions.len() - max;
            tracing::warn!(
                requested = actions.len(),
                max,
                dropped,
                "hook action batch exceeds max action depth; only the first {max} will be executed, the remaining {dropped} are discarded"
            );
            actions.into_iter().take(max).collect::<Vec<_>>()
        } else {
            actions
        };
        let mut forwarded = Vec::with_capacity(actions.len());
        for action in actions {
            // All three variants carry a `session_id` and collapse to the
            // same daemon call (`open_session` is idempotent), so dispatch
            // only the log label here and share the request/call/error path
            // below. For `SendPrompt`, the `text` and `chain_depth` fields
            // are not consumed daemon-side — they ride along on the
            // forwarded action so the TUI can echo the prompt and relay the
            // depth back via `RuntimeTurnRequest.chain_depth`.
            let (action_name, session_id) = match &action {
                HookAction::CreateSession { session_id } => ("create_session", session_id),
                HookAction::SwitchSession { session_id } => ("switch_session", session_id),
                HookAction::SendPrompt { session_id, .. } => ("send_prompt", session_id),
            };
            let request = SessionOpenRequest {
                session_id: session_id.clone(),
                conversation_id: session_id.clone(),
                sender_id: "hook_action".to_string(),
                entry: GatewayEntryContext::default(),
                channel: None,
                channel_instance_id: None,
                llm: None,
                workspace: None,
                skills: None,
                client_id: Some(daemon_hook_principal(action_name)),
                client_pid: None,
                client_hostname: None,
            };
            let should_forward = match self.control_plane.open_session(request).await {
                Ok(_) => true,
                Err(error) => {
                    tracing::warn!(
                        action = action_name,
                        session_id = %session_id,
                        error = %error,
                        "daemon-side {action_name} action failed; not forwarding to TUI",
                    );
                    false
                }
            };
            if should_forward {
                forwarded.push(action);
            }
        }
        forwarded
    }
}
