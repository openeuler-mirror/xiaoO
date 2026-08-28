//! /api/v1/runtimes/input 的 SSE 事件模型。
//!
//! 【L1 支撑类型层；v1.2 落地】
//!
//! 本模块提供强类型模型 [`RuntimeSseEvent`]，是 xiaoo（HTTP client）消费
//! daemon SSE 流的**客户端协议面**。字段名/结构与 daemon 的 `SseStreamEvent`
//! 序列化逐字段对齐（对齐基线：daemon 移出本仓前的
//! `apps/serverside/src/httpserver/sse_sink.rs:20-129`）。
//! daemon（服务端/生产者侧）已迁往独立代码仓；本仓只保留反序列化消费方。
//!
//! 事件词汇的语义消费方式由客户端决定——远程会话实现把
//! `RuntimeSseEvent` 映射为各自的应用层事件模型（原门面 `TurnEvent` 已
//! 随 pure-runtime 重构移除，协议面不再耦合具体客户端类型）。
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

use agent_types::hook::HookAction;
use agent_types::interaction::InteractionRequest;
use agent_types::llm::ChatMessage;
use serde::{Deserialize, Serialize};

use crate::plan::TodoSnapshotItem;

/// SSE 流中的一条事件。`agent_id` 区分主 Agent 与 Subagent 泳道。
///
/// 字段名/结构与 daemon `SseStreamEvent` 逐字段对齐——daemon 序列化什么，
/// 本类型就反序列化什么。本类型同时实现 `Serialize`，供 daemon 仓未来
/// 复用本模块序列化，届时协议单点定义。
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
    InteractionRequested { request: InteractionRequest },
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

/// 解析一行 SSE `data:` 载荷。
///
/// - 已知事件类型 → `Ok(Some(event))`
/// - 未知事件类型 → `Ok(None)`（向前兼容；daemon 新增类型时旧客户端跳过）
/// - 空/null 载荷 → `Ok(None)`
/// - JSON 语法错误或字段类型错误 → `Err(serde_json::Error)`
pub fn parse_sse_data(data: &str) -> Result<Option<RuntimeSseEvent>, serde_json::Error> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(None);
    }
    // 空字符串与字面 `null` 已在前置短路，避免 "Value DOM → typed" 的二次
    // 解析开销。未知事件类型经 `#[serde(other)]` 落到 `Unknown`，再映射为 None。
    let event: RuntimeSseEvent = serde_json::from_str(trimmed)?;
    match event {
        RuntimeSseEvent::Unknown => Ok(None),
        other => Ok(Some(other)),
    }
}

#[cfg(test)]
#[path = "sse/tests.rs"]
mod tests;
