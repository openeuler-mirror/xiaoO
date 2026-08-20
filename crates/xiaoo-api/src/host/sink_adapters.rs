//! 内部 sink 适配器：把 6 个 sink trait 适配成 `TurnEvent` 通道事件。
//!
//! 提炼自 endside 现有代码：
//! - `ChannelLoopEventSink` / `ChannelToolEventSink`（`session_sink.rs`）
//! - `ChannelInteractionHandle`（`session_interaction.rs`）
//! - `ChannelPendingUserMessages`（`session.rs:146-235`）
//! - `CliEventSink` 的文本快照 → 增量 diff 逻辑（`cli/mod.rs:50-107`）
//!
//! 背压语义：本 API 不另行发明语义，以 endside 现有 `session_sink.rs`
//! 的通道实现为基准——使用 `tokio::sync::mpsc::unbounded_channel`（**无界**），
//! send 失败（receiver 已 drop）静默丢弃（`let _ = ... .send(...)`）。无界
//! 通道意味着生产者（agent 内核 sink 回调）不会反压消费者（`TurnHandle::next_event`），
//! 消费方读取慢时事件在通道中无界积压。这是 endside 现有行为，本 API 与现状一致。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use agent_contracts::{InteractionHandle, LoopEventSink, ToolEventSink};
use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::llm::{ChatMessage, ContentBlock, MessageRole};
use xiaoo_core::PendingUserMessageSource;

use super::turn_handle::{InteractionResponder, TurnEvent};

// ===========================================================================
// InternalLoopEventSink
// ===========================================================================

/// `LoopEventSink` → `TurnEvent` 适配器。
///
/// 文本快照 → 增量 diff 逻辑提炼自 endside `cli/mod.rs:50-107` 的 `CliEventSink`：
/// 维护 `active_assistant_agent` 与 `last_assistant_text_len`，相邻同 agent 的
/// 快照做 char-boundary-aware 切片得到 `Text` delta；agent 切换时 reset 长度
/// 但 delta 仍为全文（与现状一致）。
///
/// 同时透出 `AssistantSnapshot`（与 `Text` 增量二选一消费）。
pub(crate) struct InternalLoopEventSink {
    event_tx: mpsc::UnboundedSender<TurnEvent>,
    state: Mutex<LoopSinkState>,
}

struct LoopSinkState {
    active_assistant_agent: Option<String>,
    last_assistant_text_len: usize,
    active_thinking_agent: Option<String>,
    last_thinking_text_len: usize,
}

impl InternalLoopEventSink {
    pub(crate) fn new(event_tx: mpsc::UnboundedSender<TurnEvent>) -> Self {
        Self {
            event_tx,
            state: Mutex::new(LoopSinkState {
                active_assistant_agent: None,
                last_assistant_text_len: 0,
                active_thinking_agent: None,
                last_thinking_text_len: 0,
            }),
        }
    }
}

impl LoopEventSink for InternalLoopEventSink {
    fn on_turn_start(&self, _agent_id: &AgentId, _turn: u32) {
        // TurnEvent 无 TurnStart 变体；仅重置 assistant/thinking 流状态
        // （同 agent 的下一 turn 也算"新流"，下次 on_assistant_message 走全文 delta）。
        if let Ok(mut state) = self.state.lock() {
            state.active_assistant_agent = None;
            state.last_assistant_text_len = 0;
            state.active_thinking_agent = None;
            state.last_thinking_text_len = 0;
        }
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        let mut state = self
            .state
            .lock()
            .expect("loop sink state mutex should not be poisoned");
        let is_same_stream = state.active_assistant_agent.as_deref() == Some(&agent_id.0);
        let prev_len = if is_same_stream {
            state.last_assistant_text_len
        } else {
            // 流切换（agent 变化或首个快照）——与 CliEventSink 一致，prev_len=0
            0
        };

        // Text delta（CliEventSink 模式：char-boundary-aware 切片）
        if prev_len < text.len() && text.is_char_boundary(prev_len) {
            let delta = text[prev_len..].to_string();
            let _ = self.event_tx.send(TurnEvent::Text {
                agent_id: Some(agent_id.clone()),
                delta,
            });
        }

        // AssistantSnapshot（原样透出，供需要完整消息形态的渲染方使用）
        let message = ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        };
        let _ = self.event_tx.send(TurnEvent::AssistantSnapshot {
            agent_id: Some(agent_id.clone()),
            message,
        });

        state.active_assistant_agent = Some(agent_id.0.clone());
        state.last_assistant_text_len = text.len();
    }

    fn on_assistant_reasoning(&self, agent_id: &AgentId, text: &str) {
        let mut state = self
            .state
            .lock()
            .expect("loop sink state mutex should not be poisoned");
        let is_same_stream = state.active_thinking_agent.as_deref() == Some(&agent_id.0);
        let prev_len = if is_same_stream {
            state.last_thinking_text_len
        } else {
            0
        };

        if prev_len < text.len() && text.is_char_boundary(prev_len) {
            let delta = text[prev_len..].to_string();
            let _ = self.event_tx.send(TurnEvent::Thinking {
                agent_id: Some(agent_id.clone()),
                delta,
            });
        }
        // TurnEvent 无 ThinkingSnapshot 变体；不透出快照形态。

        state.active_thinking_agent = Some(agent_id.0.clone());
        state.last_thinking_text_len = text.len();
    }

    fn on_tool_result(
        &self,
        agent_id: &AgentId,
        event: &ToolResultEvent,
    ) {
        let _ = self.event_tx.send(TurnEvent::ToolResult {
            agent_id: Some(agent_id.clone()),
            event: event.clone(),
        });
    }

    fn on_loop_end(&self, agent_id: &AgentId, summary: &LoopEndSummary) {
        // End 变体携带 agent_id（缺失会导致子泳道 is_running 不归位）。
        let _ = self.event_tx.send(TurnEvent::End {
            agent_id: Some(agent_id.clone()),
            summary: summary.clone(),
        });
    }
}

// ===========================================================================
// InternalToolEventSink
// ===========================================================================

/// `ToolEventSink` → `TurnEvent::Tool` 适配器。
///
/// `ToolLifecycleEvent` 的 `AgentScoped` 变体携带 agent_id；非 scoped 的事件
/// agent_id 为 `None`（类型为 `Option<AgentId>`）。
pub(crate) struct InternalToolEventSink {
    event_tx: mpsc::UnboundedSender<TurnEvent>,
}

impl InternalToolEventSink {
    pub(crate) fn new(event_tx: mpsc::UnboundedSender<TurnEvent>) -> Self {
        Self { event_tx }
    }
}

impl ToolEventSink for InternalToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        let (agent_id, inner_event) = unscope_tool_event(event);
        let _ = self.event_tx.send(TurnEvent::Tool {
            agent_id,
            event: inner_event,
        });
    }
}

/// 把 `ToolLifecycleEvent::AgentScoped` 拆出 agent_id；其余变体 agent_id 为 None。
fn unscope_tool_event(event: ToolLifecycleEvent) -> (Option<AgentId>, ToolLifecycleEvent) {
    match event {
        ToolLifecycleEvent::AgentScoped { agent_id, event } => {
            let (inner_agent_id, inner_event) = unscope_tool_event(*event);
            // 内层 scoped 优先（最内层 agent_id）
            (Some(inner_agent_id.unwrap_or(agent_id)), inner_event)
        }
        other => (None, other),
    }
}

// ===========================================================================
// InternalInteractionHandle
// ===========================================================================

/// `InteractionHandle` 适配器：把 `ask()` 转成 `TurnEvent::Interaction` 事件 +
/// oneshot 等待应答。
///
/// 缺省应答（responder 被 drop 未应答 / turn 被取消导致 ask future 被 drop）：
/// 与 endside `ChannelInteractionHandle::ask` 一致——`Confirm` 默认拒绝、
/// `TextInput` 默认 `value=None`、`Choice` 默认 `value=None`。
pub(crate) struct InternalInteractionHandle {
    event_tx: mpsc::UnboundedSender<TurnEvent>,
}

impl InternalInteractionHandle {
    pub(crate) fn new(event_tx: mpsc::UnboundedSender<TurnEvent>) -> Self {
        Self { event_tx }
    }
}

#[async_trait]
impl InteractionHandle for InternalInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let (tx, rx) = oneshot::channel();
        let responder = InteractionResponder { sender: tx };
        let _ = self.event_tx.send(TurnEvent::Interaction {
            request: request.clone(),
            responder,
        });

        match rx.await {
            Ok(response) => response,
            Err(_) => default_interaction_response(request),
        }
    }
}

/// 缺省应答：responder 未应答或 turn 被取消时使用。
/// 对照 endside `session_interaction.rs:143-150`。
fn default_interaction_response(request: &InteractionRequest) -> InteractionResponse {
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: None,
            display_value: None,
        },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice { value: None },
    }
}

// ===========================================================================
// InternalPendingUserMessages
// ===========================================================================

/// `PendingUserMessageSource` 适配器：从 `TurnHandle::queue_input` 写入的
/// `Arc<Mutex<VecDeque<String>>>` 中 drain 出排队输入。
pub(crate) struct InternalPendingUserMessages {
    pending: Arc<Mutex<VecDeque<String>>>,
}

impl InternalPendingUserMessages {
    pub(crate) fn new(pending: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self { pending }
    }
}

#[async_trait]
impl PendingUserMessageSource for InternalPendingUserMessages {
    async fn drain_pending_user_messages(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
}
