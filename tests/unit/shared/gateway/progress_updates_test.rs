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
        args_preview: String::new(),
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
