//! `TurnHandle` / `TurnEvent` / `InteractionResponder` / `TurnOptions`。
//!
//! 【L0 门面层】一轮对话的流式句柄与事件流。事件词汇与远程 SSE 模型
//! 对齐，本地/远程消费代码可以共用。
//!
//! 提炼自 endside 现有代码（逐行对照）：
//! - 通道适配 ⇐ `session_sink.rs` / `session_interaction.rs` / `cli/mod.rs:50-107`
//! - 文本快照 → 增量 diff 逻辑 ⇐ `CliEventSink`（`cli/mod.rs:50-107`）
//! - `InteractionResponder` 缺省应答语义 ⇐ `ChannelInteractionHandle`（`session_interaction.rs:143-150`）

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use agent_types::chat::CommandContext;
use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::llm::ChatMessage;
use xiaoo_shared::gateway::{
    AppTurnResult, GatewayEntryContext, LlmRuntimeConfig, SessionServiceError,
};
use xiaoo_shared::plan::SpawnSubagentMetadata;

// ===========================================================================
// TurnEvent
// ===========================================================================

/// 一轮对话中的流式事件。词汇与远程 SSE 模型（`xiaoo_api::sse::RuntimeSseEvent`）
/// 对齐，本地/远程消费代码可以共用。`agent_id` 区分主 Agent 与 Subagent 泳道。
///
/// ## plan/diff 事件本地暂缺
///
/// 本地内核目前不产出结构化 plan/diff（daemon 侧靠 `daemon_sinks.rs` 装饰器预
/// 计算，不进 API；本地 TUI 靠前端反解析）。需要 plan/diff 的调用方从 `Tool`
/// 事件用 [`crate::plan::todo_snapshot_from_tool_args`] /
/// [`crate::diff::file_change_delta_from_tool_args`]（L1）反解析——与现状一致。
/// `#[non_exhaustive]` 预留 `PlanUpdate` / `FileChange` 变体（未来由内核
/// 产出），届时与 SSE 模型对齐。
///
/// ## hook_actions 不自动执行
///
/// `AppTurnResult.hook_actions` 由调用方决定是否执行，与现状一致（保持
/// daemon/TUI 各自的执行策略空间）。
#[non_exhaustive]
pub enum TurnEvent {
    /// 助手文本增量。由内部适配器从 `LoopEventSink::on_assistant_message` 的
    /// 渐进快照 diff 出增量（逻辑提炼自 TUI 现有渲染路径 `cli/mod.rs:50-107`）。
    Text {
        /// 主 Agent 或 Subagent 泳道 id；`None` 表示未知。
        agent_id: Option<AgentId>,
        /// 自上次同 agent 快照以来新增的文本。
        delta: String,
    },
    /// 思考文本增量。同 `Text` 的 diff 逻辑。
    Thinking {
        /// 同 `Text::agent_id`。
        agent_id: Option<AgentId>,
        /// 自上次同 agent 思考快照以来新增的文本。
        delta: String,
    },
    /// 助手消息渐进快照（`LoopEventSink::on_assistant_message` 原样透出）。
    /// 供需要完整消息形态的渲染方使用；与 `Text` 增量二选一消费。
    AssistantSnapshot {
        /// 同 `Text::agent_id`。
        agent_id: Option<AgentId>,
        /// 当前完整消息快照。
        message: ChatMessage,
    },
    /// 工具生命周期事件（开始/运行/结束，含 args_preview）。
    Tool {
        /// 工具所属 agent；`None` 表示非 scoped 事件。
        agent_id: Option<AgentId>,
        /// 工具生命周期事件。
        event: ToolLifecycleEvent,
    },
    /// 工具结果事件。
    ToolResult {
        /// 工具所属 agent。
        agent_id: Option<AgentId>,
        /// 工具结果事件。
        event: ToolResultEvent,
    },
    /// 交互请求（工具审批/提问）。必须经 `responder` 应答，否则按缺省语义
    /// 处理（`Confirm` 默认拒绝、`TextInput` 默认 `value=None`、`Choice` 默认
    /// `value=None`）。
    Interaction {
        /// 交互请求负载。
        request: InteractionRequest,
        /// 应答器。消费 self 后只能应答一次。
        responder: InteractionResponder,
    },
    /// Subagent 泳道元数据。**本地模式不产出**（需要从 `Tool` 事件反解析，
    /// 与现状一致）；远程模式由 daemon 转发。
    SubagentMeta {
        /// Subagent 元数据。
        meta: SpawnSubagentMetadata,
    },
    /// 本轮收尾摘要（`LoopEndSummary`）。终值仍以 [`TurnHandle::result`] 为准。
    ///
    /// `agent_id` 标识收尾 agent 的泳道。该字段不可缺省：若用空值填充，TUI
    /// 桥接函数会让子泳道 `is_running` 永不归位。现与 `RuntimeSseEvent::LoopEnd`
    /// 对齐，由 sink 适配器从 `on_loop_end` 的 `agent_id` 参数透出真实值。
    End {
        /// 收尾 agent 的泳道 id；`None` 表示未知。
        agent_id: Option<AgentId>,
        /// 收尾摘要。
        summary: LoopEndSummary,
    },
}

// ===========================================================================
// InteractionResponder
// ===========================================================================

/// 交互请求的应答器。持有 `oneshot` 发送端，调用 [`respond`](Self::respond)
/// 消费 self 后只能应答一次。
///
/// **缺省语义**：responder 被 drop（未应答）或 turn 被取消时，内部
/// `InteractionHandle::ask` 返回缺省应答——`Confirm` 默认 `allowed=false`、
/// `TextInput` 默认 `value=None`、`Choice` 默认 `value=None`。与 endside
/// `ChannelInteractionHandle::ask` 一致（`session_interaction.rs:143-150`）。
pub struct InteractionResponder {
    pub(crate) sender: tokio::sync::oneshot::Sender<InteractionResponse>,
}

impl InteractionResponder {
    /// 应答一次交互请求（消费 self，只能应答一次）。
    /// 重复应答或 responder 已被 drop 时静默失败。
    pub fn respond(self, response: InteractionResponse) {
        let _ = self.sender.send(response);
    }
}

// ===========================================================================
// TurnHandle
// ===========================================================================

/// 一轮对话的流式句柄：事件流 + 取消 + 追加输入 + 结果。
///
/// ## 并发与生命周期
///
/// - **单消费者**：`next_event(&mut self)` 同一时刻只能一个调用方持有 `&mut`。
/// - `cancel` / `queue_input` 用 `&self`，可从事件循环外并发触发。
/// - **drop TurnHandle 不取消 turn**——turn 继续跑完，结果丢弃。这与
///   "drop 即取消"相比，与现状 TUI 后台 turn 行为一致。
/// - `result(self)` 消费 self；可在任意时刻调用；未消费的剩余事件被内部排空
///   （receiver drop 后，未来的 sink 回调静默失败）。
///
/// ## 背压语义
///
/// 本 API 以 endside `session_sink.rs` 现有通道实现为基准——使用
/// `tokio::sync::mpsc::unbounded_channel`（**无界**）。生产者（agent 内核
/// sink 回调）不反压消费者（`next_event`）；消费方读取慢时事件在通道中
/// 无界积压。这是 endside 现有行为，本 API 与现状一致（背压纪律）。
pub struct TurnHandle {
    /// 事件通道接收端。直接持有（非 Mutex），`next_event(&mut self)` 独占访问。
    pub(crate) event_rx: Option<mpsc::UnboundedReceiver<TurnEvent>>,
    /// 取消令牌（与 agent loop 共享）。`CancellationToken::cancel` 是 `&self`。
    pub(crate) cancel_token: CancellationToken,
    /// 排队输入的待消费队列（与 `InternalPendingUserMessages` 共享）。
    pub(crate) pending_user_messages: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// turn 任务句柄。`Option` 因 `result(self)` 消费 self 后 take。
    pub(crate) task_handle: Option<
        tokio::task::JoinHandle<Result<AppTurnResult, SessionServiceError>>,
    >,
    /// 同会话单活动 turn 锁。drop TurnHandle 时释放锁。
    pub(crate) _active_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl TurnHandle {
    /// 取下一条事件。事件顺序与内核 sink 回调顺序一致。
    ///
    /// 返回 `None` 的条件：当事件通道的所有发送端被 drop 时返回 `None`。但消费方
    /// **不可依赖此条件退出循环**——`AppBootstrap::from_session_components_…`
    /// spawn 的 orphan reaper 后台任务持有 `Arc<CoreBackedSessionService>` → 持有
    /// `runtime_resolver` → 持有 `bindings` → 持有 sink 适配器 → 持有 `event_tx`
    /// 克隆，**只要 deps 还活着通道就永不关闭**，`next_event()` 永不返回 `None`。
    ///
    /// 正确模式：消费方在收到 [`TurnEvent::End`] 后立即 `break` 并调
    /// [`result()`](Self::result)，**不要**用 `while let Some(ev) = next_event().await`
    /// 等通道关闭——这会让循环永不退出。
    ///
    /// `&mut self` 签名在类型层面强制单消费者。
    pub async fn next_event(&mut self) -> Option<TurnEvent> {
        match &mut self.event_rx {
            Some(rx) => rx.recv().await,
            None => None,
        }
    }

    /// 请求取消本轮（触发 `CancellationToken`）。取消结果经 `result()` 返回
    /// （沿用底层 `run_turn_with_interaction` 的取消语义）。
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// turn 进行中追加排队输入（映射 `PendingUserMessageSource`）。
    pub fn queue_input(&self, text: impl Into<String>) {
        if let Ok(mut pending) = self.pending_user_messages.lock() {
            pending.push_back(text.into());
        }
    }

    /// 等待 turn 结束并取终值。可在任意时刻调用；未消费的剩余事件被内部排空。
    ///
    /// 调用后 `self` 被消费，`next_event` 不再可用。
    pub async fn result(self) -> Result<AppTurnResult, SessionServiceError> {
        let mut this = self;
        // 先 take event_rx 并 drop，让未来 send 静默失败
        let _rx = this.event_rx.take();
        drop(_rx);
        let handle = this
            .task_handle
            .take()
            .expect("result() called twice or after take");
        match handle.await {
            Ok(result) => result,
            Err(join_error) => Err(SessionServiceError::CoreRun {
                message: format!("turn task panicked: {join_error}"),
            }),
        }
    }
}

// ===========================================================================
// TurnOptions
// ===========================================================================

/// 单轮覆盖项。`Session::run_turn_with(text, options)` 用它覆盖派生时缓存的
/// `AppTurnRequest` 字段。所有字段可选；未设置的字段保持缓存值。
///
/// 字段对应 `AppTurnRequest`（`turns.rs:123-168`）。
#[derive(Clone, Debug, Default)]
pub struct TurnOptions {
    pub(crate) reasoning_effort: Option<agent_types::ReasoningEffort>,
    pub(crate) llm: Option<LlmRuntimeConfig>,
    pub(crate) command_context: Option<CommandContext>,
    pub(crate) entry: Option<GatewayEntryContext>,
}

impl TurnOptions {
    /// 构造空 `TurnOptions`（所有字段 None，保持缓存值）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 单轮推理力度覆盖（对应 `AppTurnRequest.reasoning_effort`）。
    pub fn reasoning_effort(mut self, effort: agent_types::ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    /// 单轮 LLM 覆盖（对应 `AppTurnRequest.llm`：provider/model/api_base/
    /// api_key_env/api_key）。
    pub fn llm(mut self, llm: LlmRuntimeConfig) -> Self {
        self.llm = Some(llm);
        self
    }

    /// slash 命令元数据（对应 `AppTurnRequest.command_context`，触发
    /// `*.Chat.command.before` hooker）。
    pub fn command_context(mut self, ctx: CommandContext) -> Self {
        self.command_context = Some(ctx);
        self
    }

    /// 覆盖入口来源标记（对应 `AppTurnRequest.entry`）。
    pub fn entry(mut self, entry: GatewayEntryContext) -> Self {
        self.entry = Some(entry);
        self
    }
}
