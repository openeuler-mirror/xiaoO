use std::sync::Arc;

use agent_contracts::HookActionSink;
use agent_types::hook::HookAction;
use async_trait::async_trait;
use xiaoo_shared::gateway::{GatewayEntryContext, SessionControlPlane, SessionOpenRequest};

/// Daemon-side [`HookActionSink`] that executes plugin-requested actions
/// against the daemon's own `SessionControlPlane` before forwarding them to
/// the TUI via the SSE `Done` event.
///
/// - `CreateSession` / `SwitchSession`: calls `open_session` with the
///   requested `session_id` (idempotent resume), then forwards the action
///   to the TUI so it can switch focus.
/// - `SendPrompt`: calls `open_session` to ensure the target session
///   exists (the actual turn is started by the TUI, which POSTs to
///   `/api/v1/runtimes/input` so it can stream the response back). The
///   daemon-stamped `chain_depth` rides along on the forwarded action; the
///   TUI relays it back via `RuntimeTurnRequest.chain_depth` so the daemon
///   can enforce the cross-turn depth cap.
pub struct DaemonHookActionSink {
    control_plane: Arc<dyn SessionControlPlane>,
}

impl DaemonHookActionSink {
    pub fn new(control_plane: Arc<dyn SessionControlPlane>) -> Self {
        Self { control_plane }
    }
}

#[async_trait]
impl HookActionSink for DaemonHookActionSink {
    async fn execute_on_daemon(&self, actions: Vec<HookAction>) -> Vec<HookAction> {
        let actions = if actions.len() > agent_contracts::MAX_ACTION_DEPTH {
            let max = agent_contracts::MAX_ACTION_DEPTH;
            let dropped = actions.len() - max;
            tracing::warn!(
                requested = actions.len(),
                max,
                dropped,
                "hook action batch exceeds max action depth; only the first {max} will be executed, the remaining {dropped} are discarded"
            );
            actions
                .into_iter()
                .take(agent_contracts::MAX_ACTION_DEPTH)
                .collect::<Vec<_>>()
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
