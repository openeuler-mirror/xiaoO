use crate::channels::{
    ChannelAdapter, ChannelProgressSection, ChannelProgressState, ChannelProgressUpdate,
    ChannelResult,
};
use agent_contracts::LoopEventSink;
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use agent_types::AgentId;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tracing::warn;

const MAX_STATUS_LINES: usize = 4;
const MAX_TOOL_LINES: usize = 4;
const MAX_LINE_CHARS: usize = 220;

#[derive(Clone)]
pub struct ChannelProgressRelayHandle {
    adapter: Arc<dyn ChannelAdapter>,
    conversation_id: String,
    reply_to_message_id: Option<String>,
    state: Arc<Mutex<ProgressRelayState>>,
}

#[derive(Default)]
struct ProgressRelayState {
    sent_message_id: Option<String>,
    last_rendered: Option<String>,
    tracker: ChannelProgressTracker,
}

#[derive(Default)]
struct ChannelProgressTracker {
    recent_statuses: VecDeque<String>,
    tool_updates: VecDeque<ToolProgressLine>,
    terminal_error: Option<String>,
    delivered: bool,
}

#[derive(Clone)]
struct ToolProgressLine {
    call_id: String,
    line: String,
}

impl ChannelProgressRelayHandle {
    pub fn new(
        adapter: Arc<dyn ChannelAdapter>,
        conversation_id: String,
        reply_to_message_id: Option<String>,
    ) -> Self {
        Self {
            adapter,
            conversation_id,
            reply_to_message_id,
            state: Arc::new(Mutex::new(ProgressRelayState::default())),
        }
    }

    pub async fn mark_failed(&self, error: &str) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state.tracker.mark_failed(error);
        }
        self.publish_current_progress().await
    }

    pub async fn mark_received(&self) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state.tracker.mark_received();
        }
        self.publish_current_progress().await
    }

    pub async fn mark_delivered(&self) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state.tracker.mark_delivered();
        }
        self.publish_current_progress().await
    }

    async fn record_turn_start(&self, turn: u32) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state.tracker.push_status(format!("规划第 {turn} 轮中..."));
        }
        self.publish_current_progress().await
    }

    async fn record_assistant_message(&self) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state
                .tracker
                .push_status("模型回复已生成，准备发送".to_string());
        }
        self.publish_current_progress().await
    }

    async fn record_tool_result(&self, event: ToolResultEvent) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state.tracker.upsert_tool(event);
        }
        self.publish_current_progress().await
    }

    async fn record_loop_end(&self, summary: LoopEndSummary) -> ChannelResult<()> {
        {
            let mut state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            state.tracker.record_loop_end(summary);
        }
        self.publish_current_progress().await
    }

    async fn publish_current_progress(&self) -> ChannelResult<()> {
        let (progress, rendered, sent_message_id) = {
            let state = self
                .state
                .lock()
                .expect("progress relay state lock poisoned");
            let progress = state.tracker.render();
            let rendered =
                serde_json::to_string(&progress).expect("channel progress update should serialize");
            if state.last_rendered.as_deref() == Some(rendered.as_str()) {
                return Ok(());
            }
            (progress, rendered, state.sent_message_id.clone())
        };

        let new_message_id = if let Some(message_id) = sent_message_id.as_deref() {
            self.adapter
                .update_progress_update(message_id, &progress)
                .await?;
            Some(message_id.to_string())
        } else {
            self.adapter
                .send_progress_update(
                    &self.conversation_id,
                    &progress,
                    self.reply_to_message_id.as_deref(),
                )
                .await?
        };

        let mut state = self
            .state
            .lock()
            .expect("progress relay state lock poisoned");
        state.last_rendered = Some(rendered);
        state.sent_message_id = new_message_id.or(state.sent_message_id.clone());
        Ok(())
    }
}

impl LoopEventSink for ChannelProgressRelayHandle {
    fn on_turn_start(&self, _agent_id: &AgentId, turn: u32) {
        let relay = self.clone();
        tokio::spawn(async move {
            if let Err(error) = relay.record_turn_start(turn).await {
                warn!("failed to publish channel progress turn-start update: {error}");
            }
        });
    }

    fn on_assistant_message(&self, _agent_id: &AgentId, _text: &str) {
        let relay = self.clone();
        tokio::spawn(async move {
            if let Err(error) = relay.record_assistant_message().await {
                warn!("failed to publish channel progress assistant update: {error}");
            }
        });
    }

    fn on_tool_result(&self, _agent_id: &AgentId, event: &ToolResultEvent) {
        let relay = self.clone();
        let event = event.clone();
        tokio::spawn(async move {
            if let Err(error) = relay.record_tool_result(event).await {
                warn!("failed to publish channel progress tool update: {error}");
            }
        });
    }

    fn on_loop_end(&self, _agent_id: &AgentId, summary: &LoopEndSummary) {
        let relay = self.clone();
        let summary = summary.clone();
        tokio::spawn(async move {
            if let Err(error) = relay.record_loop_end(summary).await {
                warn!("failed to publish channel progress loop-end update: {error}");
            }
        });
    }
}

impl ChannelProgressTracker {
    fn mark_received(&mut self) {
        self.push_status("已接收请求，正在处理".to_string());
    }

    fn mark_failed(&mut self, error: &str) {
        self.terminal_error = Some(truncate_line(error));
        self.delivered = false;
    }

    fn mark_delivered(&mut self) {
        self.terminal_error = None;
        self.delivered = true;
        self.push_status("最终回复已发送".to_string());
    }

    fn record_loop_end(&mut self, summary: LoopEndSummary) {
        self.push_status(match summary.stop_reason.as_str() {
            "complete" => "执行完成，准备发送最终回复".to_string(),
            other => format!("执行结束: {other}"),
        });
    }

    fn push_status(&mut self, status: String) {
        let normalized = truncate_line(&status);
        if self.recent_statuses.back() == Some(&normalized) {
            return;
        }
        self.recent_statuses.push_back(normalized);
        while self.recent_statuses.len() > MAX_STATUS_LINES {
            self.recent_statuses.pop_front();
        }
    }

    fn upsert_tool(&mut self, event: ToolResultEvent) {
        let status = if event.is_error { "失败" } else { "完成" };
        let line = truncate_line(&format!(
            "{status}: {} - {}",
            event.tool_name, event.output_preview
        ));

        if let Some(index) = self
            .tool_updates
            .iter()
            .position(|existing| existing.call_id == event.call_id)
        {
            self.tool_updates.remove(index);
        }

        self.tool_updates.push_front(ToolProgressLine {
            call_id: event.call_id,
            line,
        });
        while self.tool_updates.len() > MAX_TOOL_LINES {
            self.tool_updates.pop_back();
        }
    }

    fn render(&self) -> ChannelProgressUpdate {
        let state = if self.terminal_error.is_some() {
            ChannelProgressState::Failed
        } else if self.delivered {
            ChannelProgressState::Completed
        } else {
            ChannelProgressState::Running
        };

        let summary = if let Some(error) = &self.terminal_error {
            error.clone()
        } else if self.delivered {
            "最终回复已发送".to_string()
        } else if let Some(status) = self.recent_statuses.back() {
            status.clone()
        } else {
            "已接收请求，正在处理".to_string()
        };

        let mut sections = Vec::new();
        if !self.recent_statuses.is_empty() {
            sections.push(ChannelProgressSection {
                heading: "状态".to_string(),
                lines: self.recent_statuses.iter().cloned().collect(),
            });
        }
        if !self.tool_updates.is_empty() {
            sections.push(ChannelProgressSection {
                heading: "工具执行".to_string(),
                lines: self
                    .tool_updates
                    .iter()
                    .map(|update| update.line.clone())
                    .collect(),
            });
        }

        ChannelProgressUpdate {
            title: match state {
                ChannelProgressState::Running => "小欧正在处理".to_string(),
                ChannelProgressState::Completed => "小欧处理完成".to_string(),
                ChannelProgressState::Failed => "小欧执行失败".to_string(),
            },
            summary,
            state,
            sections,
        }
    }
}

fn truncate_line(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let truncated = chars.by_ref().take(MAX_LINE_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelProgressTracker;
    use crate::channels::ChannelProgressState;
    use agent_types::events::{LoopEndSummary, ToolResultEvent};

    #[test]
    fn tracker_renders_status_and_tool_sections() {
        let mut tracker = ChannelProgressTracker::default();
        tracker.push_status("解析意图完成".to_string());
        tracker.upsert_tool(ToolResultEvent {
            call_id: "call-1".to_string(),
            tool_name: "search".to_string(),
            output_preview: "找到 3 条资料".to_string(),
            is_error: false,
        });
        tracker.record_loop_end(LoopEndSummary {
            turn_count: 1,
            total_tokens: 128,
            stop_reason: "complete".to_string(),
        });

        let rendered = tracker.render();

        assert_eq!(rendered.state, ChannelProgressState::Running);
        assert_eq!(rendered.title, "小欧正在处理");
        assert_eq!(rendered.sections.len(), 2);
        assert_eq!(rendered.sections[0].heading, "状态");
        assert_eq!(rendered.sections[1].heading, "工具执行");
    }
}
