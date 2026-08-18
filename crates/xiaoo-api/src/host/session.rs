//! `Session`：会话句柄。
//!
//! 【L0 门面层】绑定 `session_id` 与派生时缓存的 `(SessionOpenRequest,
//! HostedSessionRuntimeConfig)`。`send` / `run_turn` / `run_turn_with` /
//! `update_options` / `export` / `close` 在本阶段全部落地。
//!
//! 提炼纪律：对照 `refactor.md` §3.3.4 / §3.3.8，**不新增行为**。

use std::sync::Arc;

use xiaoo_shared::gateway::{
    AppTurnRequest, AppTurnResult, HostedSessionRuntimeConfig, HostedSessionRuntimeResolver,
    SessionOpenRequest, SessionRecord, SessionRuntimeBindings, SessionServiceError, SessionStore,
};

use super::local_session_host::HostInner;
use super::options::SessionOptions;
use super::sink_adapters::{
    InternalInteractionHandle, InternalLoopEventSink, InternalPendingUserMessages,
    InternalToolEventSink,
};
use super::turn_handle::{TurnHandle, TurnOptions};
use xiaoo_shared::gateway::bootstrap::AppBootstrap;

/// 会话句柄：绑定 `session_id` 与派生时缓存的 `(SessionOpenRequest,
/// HostedSessionRuntimeConfig)`。
///
/// 内部持 `Arc`，`Clone` 廉价；同会话单活动 turn（§3.3.7）。`close(self)` 显式
/// 关闭；drop 不隐式关闭。本地模式租约主体即本进程，调用方无需心跳。
pub struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    host: Arc<HostInner>,
    session_id: String,
    /// 派生时缓存的 `SessionOpenRequest`。`update_options` 会替换它。
    /// 用 `tokio::sync::Mutex` 因为派生是 async（不能在 sync Mutex 跨 await 持锁）。
    open_request: tokio::sync::Mutex<SessionOpenRequest>,
    /// 派生时缓存的 `HostedSessionRuntimeConfig`。`update_options` 会替换它。
    runtime_config: tokio::sync::Mutex<HostedSessionRuntimeConfig>,
    /// 单活动 turn 锁（§3.3.7：同一 Session 同时只允许一个活动 turn）。
    /// `Arc<Mutex<>>` 让 `lock_owned()` 返回 `OwnedMutexGuard` 可移动到 TurnHandle。
    active_turn: Arc<tokio::sync::Mutex<()>>,
}

impl Session {
    pub(crate) fn new(
        host: Arc<HostInner>,
        session_id: String,
        open_request: SessionOpenRequest,
        runtime_config: HostedSessionRuntimeConfig,
    ) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                host,
                session_id,
                open_request: tokio::sync::Mutex::new(open_request),
                runtime_config: tokio::sync::Mutex::new(runtime_config),
                active_turn: Arc::new(tokio::sync::Mutex::new(())),
            }),
        }
    }

    /// 本会话的 id。
    pub fn id(&self) -> &str {
        &self.inner.session_id
    }

    /// 非流式：跑一轮对话直到结束，返回终值（消息、token 统计、hook_actions）。
    ///
    /// 等价于 `run_turn + 丢弃事件 + result()`。交互请求按 `InteractionHandle`
    /// 缺省语义处理（本阶段不注入 InteractionHandle，工具的 `ask_user_question` 会
    /// 失败——"默认拒绝"的等价表达）。
    ///
    /// 内部：每轮新建 resolver + empty bindings + `AppBootstrap` →
    /// `service.run_turn(request)`。对照 endside `cli/entry.rs:1041-1050`
    /// （CLI 也用 empty bindings）与 `session_core.rs:169-295`（TUI 的 per-turn
    /// bootstrap；channel 适配器部分**不**下沉，run_turn 阶段 2.3 实现）。
    pub async fn send(&self, text: impl Into<String>) -> Result<AppTurnResult, SessionServiceError> {
        let text = text.into();
        let runtime_config = self.inner.runtime_config.lock().await.clone();
        let open_request = self.inner.open_request.lock().await.clone();

        // `into_turn_request` 携带 entry/channel/client_id 等租约字段，并设置 text。
        // 对照 endside `runtime_request.rs:347-376` 的 turn_request 构造。
        let request = open_request.into_turn_request(text);

        self.run_turn_inner(request, runtime_config, SessionRuntimeBindings::default())
            .await
    }

    /// 流式：启动一轮对话，立即返回 [`TurnHandle`]。后续通过
    /// `TurnHandle::next_event` 消费事件流、`cancel` / `queue_input` 控制本轮、
    /// `result` 取终值。
    ///
    /// 内部：派生缓存的 `runtime_config` + 内部 bindings（六 sink 的通道适配
    /// ⇐ `session_sink.rs` / `session_interaction.rs` / `cli/mod.rs:50-107` 提炼）→
    /// 每 turn 新建 resolver + `AppBootstrap` → 后台 task 调
    /// `service.run_turn_with_interaction(request, None, None, None, Some(token), None)`
    /// （resolver 的 bindings 是 agent loop 实际消费的 bindings；cancel_token 经
    /// 参数覆盖以确保 agent loop 收到）。
    pub async fn run_turn(
        &self,
        text: impl Into<String>,
    ) -> Result<TurnHandle, SessionServiceError> {
        self.run_turn_with(text, TurnOptions::new()).await
    }

    /// 同 [`run_turn`](Self::run_turn)，附单轮覆盖项 `options`。
    pub async fn run_turn_with(
        &self,
        text: impl Into<String>,
        options: TurnOptions,
    ) -> Result<TurnHandle, SessionServiceError> {
        // 同会话单活动 turn（§3.3.7）—— 持锁到 turn 结束。锁在 `TurnHandle`
        // 被 drop 时释放（`_active_guard` 持有 `OwnedMutexGuard` 直到 TurnHandle
        // 消亡）。
        //
        // 注：把 guard 移入 TurnHandle 是为了支持"run_turn 后立刻 next_event"
        // 的常见用法。drop TurnHandle 不取消 turn，但释放 active_turn 锁。
        let active_guard = self.inner.active_turn.clone().lock_owned().await;

        let text = text.into();
        let runtime_config = self.inner.runtime_config.lock().await.clone();
        let open_request = self.inner.open_request.lock().await.clone();

        // 应用 TurnOptions 覆盖项到 AppTurnRequest
        let mut request = open_request.into_turn_request(text);
        if let Some(reasoning_effort) = options.reasoning_effort {
            request.reasoning_effort = reasoning_effort;
        }
        if let Some(llm) = options.llm {
            request.llm = Some(llm);
        }
        if let Some(command_context) = options.command_context {
            request.command_context = Some(command_context);
        }
        if let Some(entry) = options.entry {
            request.entry = entry;
        }

        // 构造事件通道 + 取消令牌 + 排队输入队列
        let (event_tx, event_rx) =
            tokio::sync::mpsc::unbounded_channel::<super::turn_handle::TurnEvent>();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let pending_user_messages: Arc<std::sync::Mutex<std::collections::VecDeque<String>>> =
            Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));

        // 构造 bindings（sink 适配器全部 baked in；cancel_token 经
        // run_turn_with_interaction 参数覆盖，故 bindings 中留 None）
        let bindings = SessionRuntimeBindings {
            loop_event_sink: Some(Arc::new(InternalLoopEventSink::new(event_tx.clone()))),
            tool_event_sink: Some(Arc::new(InternalToolEventSink::new(event_tx.clone()))),
            interaction_handle: Some(Arc::new(InternalInteractionHandle::new(event_tx.clone()))),
            channel_file_sender: None,
            pending_user_messages: Some(Arc::new(InternalPendingUserMessages::new(
                pending_user_messages.clone(),
            ))),
            cancel_token: None,
        };

        // 后台 task：构造 resolver + bootstrap → service.run_turn_with_interaction
        let session_store = self.inner.host.session_store.clone();
        let backend_manager = self.inner.host.backend_manager.clone();
        let memory_automation = self.inner.host.memory_automation.clone();
        let interaction_timeout = self.inner.host.interaction_timeout;
        let cancel_token_for_task = cancel_token.clone();

        let task_handle = tokio::spawn(async move {
            let hooker_config = runtime_config.hooker.clone();
            let resolver = Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
            let deps = match AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
                session_store,
                resolver,
                hooker_config,
                backend_manager,
                memory_automation,
                interaction_timeout,
            ) {
                Ok(d) => d,
                Err(e) => {
                    // bootstrap 失败：event_tx drop 后 receiver 返回 None；
                    // result() 返回 Err。不通过 event 流传递错误（§3.3.5 无
                    // Error 变体）。
                    return Err(SessionServiceError::RuntimeBuild {
                        message: e.to_string(),
                    });
                }
            };

            // 调用 run_turn_with_interaction，显式传 cancellation_token；
            // 其余 5 个 sink 参数为 None（用 resolver bindings 中 baked in 的适配器）。
            deps.session_service
                .run_turn_with_interaction(
                    request,
                    None, // event_sink（用 resolver bindings）
                    None, // interaction_handle（用 resolver bindings）
                    None, // channel_file_sender
                    Some(cancel_token_for_task), // cancellation_token（覆盖 resolver bindings）
                    None, // tool_event_sink（用 resolver bindings）
                )
                .await
        });

        Ok(TurnHandle {
            event_rx: Some(event_rx),
            cancel_token,
            pending_user_messages,
            task_handle: Some(task_handle),
            _active_guard: Some(active_guard),
        })
    }

    /// 更新会话配置（重新执行 §3.3.3 派生），下一轮 turn 生效。
    /// 覆盖场景：TUI 运行中切模型、切 sandbox backend。
    pub async fn update_options(&self, options: SessionOptions) -> Result<(), SessionServiceError> {
        let (open_request, runtime_config) = options.derive().await?;
        // 注：派生可能产生新的 session_id（如果 options.session_id 是 Some 但不同）。
        // 当前实现假设 update_options 不改 session_id；若调用方传入不同的 session_id，
        // 行为是"缓存被替换但 host 的 active_session_ids 不变"——下个 turn 会
        // 用新的 session_id 查 store，若不存在则失败。这种用法不在常规场景。
        *self.inner.open_request.lock().await = open_request;
        *self.inner.runtime_config.lock().await = runtime_config;
        Ok(())
    }

    /// 导出完整 `SessionRecord`（快照保存/调试用；映射 `SessionService::export_session`）。
    pub async fn export(&self) -> Result<SessionRecord, SessionServiceError> {
        let store: Arc<dyn SessionStore> = self.inner.host.session_store.clone();
        store
            .load(&self.inner.session_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: self.inner.session_id.clone(),
            })
    }

    /// 强制关闭本会话（映射 `SessionControlPlane::force_close_session`）。
    /// Drop 不隐式关闭——生命周期显式管理，与现状一致。
    pub async fn close(self) -> Result<SessionRecord, SessionServiceError> {
        self.inner
            .host
            .lifecycle_control_plane
            .force_close_session(&self.inner.session_id)
            .await
    }

    // ---- Advanced（L2）----

    /// 完全控制版：手工构造 `AppTurnRequest` 与原生 `SessionRuntimeBindings`
    /// （六 sink 直连）。供测试替身与 advanced 装配使用。
    pub async fn run_turn_raw(
        &self,
        request: AppTurnRequest,
        bindings: SessionRuntimeBindings,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let runtime_config = self.inner.runtime_config.lock().await.clone();
        self.run_turn_inner(request, runtime_config, bindings).await
    }

    /// 内部统一 per-turn bootstrap（同步路径）：构造 resolver + `AppBootstrap` →
    /// `service.run_turn(request)`。对照 endside `session_core.rs:216-236`。
    async fn run_turn_inner(
        &self,
        request: AppTurnRequest,
        runtime_config: HostedSessionRuntimeConfig,
        bindings: SessionRuntimeBindings,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let hooker_config = runtime_config.hooker.clone();
        let resolver = Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
        let deps = AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
            self.inner.host.session_store.clone(),
            resolver,
            hooker_config,
            self.inner.host.backend_manager.clone(),
            self.inner.host.memory_automation.clone(),
            self.inner.host.interaction_timeout,
        )
        .map_err(|e| SessionServiceError::RuntimeBuild {
            message: e.to_string(),
        })?;
        deps.session_service.run_turn(request).await
    }
}
