//! /api/v1/runtimes/input 的 SSE 事件模型。
//!
//! 【L1 支撑类型层；v1.2 落地】
//!
//! 现状问题：daemon 在 `apps/serverside/src/httpserver/sse_sink.rs` 手工拼
//! JSON 事件，xiaoo 在 `apps/endside/src/gateway_api/remote.rs` 与
//! `cli/entry.rs` 两处手工解析——协议存在但没有共享类型，双方靠字符串
//! 字段名隐式耦合。
//!
//! 本模块提供强类型模型 [`RuntimeSseEvent`]，**字段名/结构与 daemon 现有
//! 序列化逐字段对齐**（对照 `apps/serverside/src/httpserver/sse_sink.rs:20-129`
//! 的 `SseStreamEvent`）。daemon 侧代码**不改**（C-1/C-5）；本模块当前由
//! xiaoo 客户端用于反序列化，daemon 拆分后应改用本模块序列化，使协议
//! 单点定义（§7）。
//!
//! 事件词汇与门面 [`crate::host::TurnEvent`]（§3.3.5）对齐——远程 Session
//! 实现（§7）把 `RuntimeSseEvent` 1:1 映射为 `TurnEvent`。
//!
//! ## 兼容性纪律
//!
//! - 本模块类型的 serde 表示是 wire 契约，任何字段增删都要与 daemon 的
//!   实际输出核对。
//! - 未知字段一律容忍（`deny_unknown_fields` 禁用）——daemon 可能新增字段，
//!   旧客户端应静默忽略。
//! - 未知事件类型经 [`parse_sse_data`] 返回 `Ok(None)`（向前兼容）——daemon
//!   可能新增事件类型，旧客户端应跳过而非报错。
//! - 字段缺省值（`#[serde(default)]`）匹配 daemon 的 `args_preview` /
//!   `detail` / `actions` / `turn_count` / `total_tokens` / `stop_reason`
//!   等backward-compat 字段。

use agent_types::common::ids::AgentId;
use agent_types::events::{ToolLifecycleEvent, ToolResultEvent};
use agent_types::hook::HookAction;
use agent_types::interaction::InteractionRequest;
use agent_types::llm::ChatMessage;
use serde::{Deserialize, Serialize};
use xiaoo_shared::plan::{SpawnSubagentMetadata, TodoSnapshotItem, TodoSnapshotUpdate};
use xiaoo_shared::session_diff::FileChangeDelta;

/// SSE 流中的一条事件。`agent_id` 区分主 Agent 与 Subagent 泳道。
///
/// 字段名/结构与 daemon `SseStreamEvent`（`apps/serverside/src/httpserver/
/// sse_sink.rs:20-129`）逐字段对齐——daemon 序列化什么，本类型就反序列化
/// 什么。本类型同时实现 `Serialize` 以便 daemon 拆分后改用本模块序列化
/// （§7），届时协议单点定义。
//
// 字段不单独写 rustdoc：每个变体的 doc 已说明语义，字段名与 daemon
// wire 契约逐字段对齐（serde 表示即文档）。允许 missing_docs 以避免
// 大量重复字段级注释（"agent_id: 主 Agent 或 Subagent 泳道 id" 等）。
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeSseEvent {
    /// turn 开始（对照 daemon `SseStreamEvent::TurnStart`）。
    TurnStart { agent_id: String, turn: u32 },
    /// 助手文本增量。`snapshot` 是当前完整快照（供需要完整消息的渲染方）。
    TextDelta {
        agent_id: String,
        delta: String,
        snapshot: String,
    },
    /// 思考文本增量。同 `TextDelta` 的 snapshot 语义。
    ThinkingDelta {
        agent_id: String,
        delta: String,
        snapshot: String,
    },
    /// 工具结果事件（flattened，对照 daemon `SseStreamEvent::ToolResult`）。
    ToolResult {
        agent_id: String,
        call_id: String,
        tool_name: String,
        output_preview: String,
        is_error: bool,
        #[serde(default)]
        args_preview: String,
    },
    /// per-call 文件变更增量（daemon 预计算，对照 `SseStreamEvent::ToolFileChange`）。
    ToolFileChange {
        call_id: String,
        file_path: String,
        additions: u32,
        deletions: u32,
    },
    /// Plan 面板快照（daemon 从 `todo_write` args 解析，对照 `SseStreamEvent::PlanUpdate`）。
    PlanUpdate {
        title: String,
        items: Vec<TodoSnapshotItem>,
    },
    /// Subagent 泳道元数据（daemon 从 `spawn_subagent` args 解析，对照
    /// `SseStreamEvent::SubagentSpawn`）。
    SubagentSpawn {
        agent_id: String,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
    },
    /// 工具生命周期事件（flattened，对照 daemon `SseStreamEvent::ToolCall`）。
    /// 只转发 `Running` 状态；终态由后续 `ToolResult` 事件承载。
    ToolCall {
        agent_id: String,
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args_preview: String,
        status: ToolCallStatus,
        #[serde(default)]
        detail: String,
    },
    /// per-agent loop 结束标记（对照 `SseStreamEvent::LoopEnd`）。
    LoopEnd {
        agent_id: String,
        #[serde(default)]
        turn_count: u32,
        #[serde(default)]
        total_tokens: usize,
        #[serde(default)]
        stop_reason: String,
    },
    /// 交互请求（审批/提问），需要客户端 POST /runtimes/interaction 应答。
    InteractionRequested {
        request: InteractionRequest,
    },
    /// 终止事件：本轮完整消息 + token 统计 + hook 动作。
    Done {
        reply: String,
        raw_reply: String,
        conversation_id: String,
        /// serde 重命名为 `runtime_id`（与 daemon `SseStreamEvent::Done` 对齐）。
        #[serde(rename = "runtime_id")]
        session_id: String,
        turn_count: u32,
        total_tokens: usize,
        prompt_tokens: u64,
        completion_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<ChatMessage>,
        stop_reason: String,
        #[serde(default)]
        actions: Vec<HookAction>,
    },
    /// 错误终止。
    Error { error: String },
    /// 取消事件（serde 重命名 `session_id` → `runtime_id`，对照 daemon）。
    Cancelled {
        #[serde(rename = "runtime_id")]
        session_id: String,
    },
    /// 未知事件类型（向前兼容）。daemon 新增事件类型时，旧客户端经
    /// [`parse_sse_data`] 收到 `Ok(None)` 而非报错。
    #[serde(other)]
    Unknown,
}

/// Wire-format mirror of `agent_types::tool::ToolExecutionStatus`，与 daemon
/// `sse_sink.rs:135-144` 的 `ToolCallStatus` 对齐。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Tool args received, executor about to run.
    Running,
    /// Executor returned successfully.
    Completed,
    /// Executor returned an error.
    Failed,
    /// Tool args were rejected by the policy layer before execution.
    Denied,
}

// ---- 便利构造器（对照 daemon 侧 forwarder 们）----
//
// 这些构造器把高层语义类型（`ToolResultEvent` / `ToolLifecycleEvent` /
// `FileChangeDelta` / `TodoSnapshotUpdate` / `SpawnSubagentMetadata`）
// 转成 wire 形态的 `RuntimeSseEvent`。daemon 拆分后应改用这些构造器
// 而非 `SseStreamEvent`，届时协议单点定义（§7）。

impl RuntimeSseEvent {
    /// 从 `ToolResultEvent` 构造 wire 形态的 `ToolResult` 事件。
    /// 对照 daemon `SseLoopEventSink::on_tool_result`（`sse_sink.rs:253-262`）。
    pub fn from_tool_result(agent_id: &AgentId, event: &ToolResultEvent) -> Self {
        Self::ToolResult {
            agent_id: agent_id.0.clone(),
            call_id: event.call_id.clone(),
            tool_name: event.tool_name.clone(),
            output_preview: event.output_preview.clone(),
            is_error: event.is_error,
            args_preview: event.args_preview.clone(),
        }
    }

    /// 从 `ToolLifecycleEvent` 构造 wire 形态的 `ToolCall` 事件。
    /// 仅 `Running`/`Pending` 状态转发；终态返回 `None`（对照 daemon
    /// `SseToolEventSink::emit`，`sse_sink.rs:471-494`）。
    pub fn from_tool_lifecycle(
        event: ToolLifecycleEvent,
        fallback_agent_id: AgentId,
    ) -> Option<Self> {
        let (agent_id, call_id, tool_name, args_preview, status, detail) =
            flatten_tool_lifecycle(event, fallback_agent_id);
        // 跳过终态——后续 ToolResult 事件承载（对照 daemon 实现）
        if status != ToolCallStatus::Running {
            return None;
        }
        Some(Self::ToolCall {
            agent_id: agent_id.0,
            call_id,
            tool_name,
            args_preview,
            status,
            detail,
        })
    }

    /// 从 `FileChangeDelta` 构造 wire 形态的 `ToolFileChange` 事件。
    /// 对照 daemon `SseDeltaForwarder::forward_delta`（`sse_sink.rs:307-316`）。
    pub fn from_file_change_delta(call_id: &str, delta: FileChangeDelta) -> Self {
        Self::ToolFileChange {
            call_id: call_id.to_string(),
            file_path: delta.file_path,
            additions: delta.additions,
            deletions: delta.deletions,
        }
    }

    /// 从 `TodoSnapshotUpdate` 构造 wire 形态的 `PlanUpdate` 事件。
    /// 对照 daemon `SsePlanForwarder::forward_plan`（`sse_sink.rs:332-339`）。
    pub fn from_plan_update(update: TodoSnapshotUpdate) -> Self {
        Self::PlanUpdate {
            title: update.title,
            items: update.items,
        }
    }

    /// 从 `SpawnSubagentMetadata` 构造 wire 形态的 `SubagentSpawn` 事件。
    /// 对照 daemon `SseSubagentMetaForwarder::forward_subagent_meta`
    /// （`sse_sink.rs:355-365`）。
    pub fn from_subagent_meta(meta: SpawnSubagentMetadata) -> Self {
        Self::SubagentSpawn {
            agent_id: meta.agent_id,
            parent_agent_id: meta.parent_agent_id,
            title: meta.title,
            description: meta.description,
            task_goal: meta.task_goal,
        }
    }
}

/// 递归解包 `AgentScoped` 层并提取 wire-format 字段。
/// 对照 daemon `sse_sink.rs:400-464` 的 `flatten_tool_lifecycle`。
fn flatten_tool_lifecycle(
    event: ToolLifecycleEvent,
    fallback_agent_id: AgentId,
) -> (AgentId, String, String, String, ToolCallStatus, String) {
    match event {
        ToolLifecycleEvent::AgentScoped { agent_id, event } => {
            flatten_tool_lifecycle(*event, agent_id)
        }
        ToolLifecycleEvent::Pending {
            call_id,
            tool_name,
            args_preview,
        }
        | ToolLifecycleEvent::Running {
            call_id,
            tool_name,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Running,
            String::new(),
        ),
        ToolLifecycleEvent::Completed {
            call_id,
            tool_name,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Completed,
            String::new(),
        ),
        ToolLifecycleEvent::Failed {
            call_id,
            tool_name,
            error,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Failed,
            error,
        ),
        ToolLifecycleEvent::Denied {
            call_id,
            tool_name,
            reason,
            args_preview,
        } => (
            fallback_agent_id,
            call_id,
            tool_name,
            args_preview,
            ToolCallStatus::Denied,
            reason,
        ),
    }
}

/// 解析一行 SSE `data:` 载荷。
///
/// - 已知事件类型 → `Ok(Some(event))`
/// - 未知事件类型 → `Ok(None)`（向前兼容；daemon 新增类型时旧客户端跳过）
/// - 空/null 载荷 → `Ok(None)`
/// - JSON 语法错误或字段类型错误 → `Err(serde_json::Error)`
pub fn parse_sse_data(data: &str) -> Result<Option<RuntimeSseEvent>, serde_json::Error> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // 先解析为 Value 以处理 null；直接 from_str::<RuntimeSseEvent>("null")
    // 会因 enum 不接受 null 而报错，故先解 Value 再分支。
    let value: serde_json::Value = serde_json::from_str(trimmed)?;
    if value.is_null() {
        return Ok(None);
    }
    let event: RuntimeSseEvent = serde_json::from_value(value)?;
    match event {
        RuntimeSseEvent::Unknown => Ok(None),
        other => Ok(Some(other)),
    }
}

#[cfg(test)]
mod tests;
