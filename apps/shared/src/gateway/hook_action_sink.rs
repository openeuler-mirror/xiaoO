//! Daemon 侧 hook 动作执行器。
//!
//! 原属 `serverside/httpserver/action_sink.rs`，整文件搬入此处。归属依据：
//! 它是"hook 动作 → 会话控制面"的执行器，依赖面
//! （`SessionControlPlane` / `SessionOpenRequest` / `GatewayEntryContext` /
//! `daemon_hook_principal`）全部在 `shared::gateway`；`shared::channels` 是
//! IM 渠道适配词汇，与 hook 动作执行无关，不放那里。
//!
//! shared 只 pub 具体类型 [`DaemonHookActionSink`]（`new(control_plane)` 构造，
//! 固有方法 `execute_on_daemon`）。router 是唯一调用方，自己持有 sink 字段并
//! 直接调 `execute_on_daemon`（"构造后值传递"不成立），故 router 字段改持
//! 具体类型 `Option<Arc<DaemonHookActionSink>>`。
//! `HookActionSink` trait 与 `MAX_ACTION_DEPTH` 常量不向应用导出——
//! 它们仅是本实现的内部细节：`execute_on_daemon` 是固有方法，不再经 trait
//! 暴露。

use std::sync::Arc;

use xiaoo_api::chat::HookAction;

use crate::gateway::{
    daemon_hook_principal, GatewayEntryContext, SessionControlPlane, SessionOpenRequest,
};

/// Daemon 侧 hook 动作执行器：在转发给 TUI 前，先针对 daemon 自己的
/// `SessionControlPlane` 执行插件请求的动作。
///
/// - `CreateSession` / `SwitchSession`：以请求的 `session_id` 调 `open_session`
///   （幂等 resume），再转发给 TUI 以便切换焦点。
/// - `SendPrompt`：调 `open_session` 确保目标会话存在（实际 turn 由 TUI 发起，
///   它 POST 到 `/api/v1/runtimes/input` 以便流式回传响应）。daemon 盖戳的
///   `chain_depth` 随转发动作同行；TUI 再经 `RuntimeTurnRequest.chain_depth`
///   回传，使 daemon 能施加跨 turn 深度上限。
pub struct DaemonHookActionSink {
    control_plane: Arc<dyn SessionControlPlane>,
}

impl DaemonHookActionSink {
    /// 以给定的会话控制面构造执行器。
    pub fn new(control_plane: Arc<dyn SessionControlPlane>) -> Self {
        Self { control_plane }
    }

    /// 在 daemon 侧执行一批 hook 动作，返回应转发给 TUI 的子集。
    ///
    /// 固有方法；不再经 `HookActionSink` trait 暴露（trait 在本实现中已无必要）。
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
