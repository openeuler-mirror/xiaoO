use super::{default_provider_list, ChatState, Input, Message};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget},
};

#[test]
fn default_provider_list_contains_latest_models_and_excludes_retired_kimi_models() {
    let providers = default_provider_list();
    let model_ids = |provider: &str| {
        providers
            .iter()
            .find(|entry| entry.name == provider)
            .unwrap_or_else(|| panic!("missing provider {provider}"))
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
    };

    let openai = model_ids("openai");
    assert!(openai.contains(&"gpt-5.6"));
    assert!(openai.contains(&"gpt-5.6-sol"));
    assert!(openai.contains(&"gpt-5.6-terra"));
    assert!(openai.contains(&"gpt-5.6-luna"));

    let anthropic = model_ids("anthropic");
    assert!(anthropic.contains(&"claude-fable-5"));
    assert!(anthropic.contains(&"claude-sonnet-5"));

    let kimi = model_ids("kimi");
    assert!(kimi.contains(&"kimi-k3"));
    assert!(!kimi.contains(&"kimi-k2-0905-preview"));
    assert!(!kimi.contains(&"kimi-latest"));

    assert!(model_ids("zhipu").contains(&"glm-5.2"));
}

#[test]
fn message_render_revision_updates_with_content_and_streaming_changes() {
    let mut message = Message::assistant_streaming();
    assert_eq!(message.render_revision, 0);

    message.set_content("hello");
    assert_eq!(message.render_revision, 1);

    message.append_content(" world");
    assert_eq!(message.render_revision, 2);

    message.set_streaming(false);
    assert_eq!(message.render_revision, 3);

    message.set_streaming(false);
    assert_eq!(message.render_revision, 3);
}

#[test]
fn input_history_walks_entries_from_newest_to_oldest() {
    let mut chat = ChatState::default();
    chat.set_input_history(vec!["first".to_string(), "second".to_string()]);

    assert!(chat.previous_input_history());
    assert_eq!(chat.input.value(), "second");

    assert!(chat.previous_input_history());
    assert_eq!(chat.input.value(), "first");

    assert!(chat.previous_input_history());
    assert_eq!(chat.input.value(), "first");
}

#[test]
fn input_history_down_restores_draft_after_latest_entry() {
    let mut chat = ChatState::default();
    chat.set_input_history(vec!["first".to_string(), "second".to_string()]);
    chat.input = Input::from("draft");

    assert!(chat.previous_input_history());
    assert_eq!(chat.input.value(), "second");

    assert!(chat.next_input_history());
    assert_eq!(chat.input.value(), "draft");
    assert_eq!(chat.input_history_cursor, None);
}

#[test]
fn input_history_ignores_transcript_messages() {
    let mut chat = ChatState::default();
    chat.messages.push(Message::system("system"));
    chat.messages.push(Message::assistant_streaming());
    chat.messages.push(Message::user("not history"));
    chat.input = Input::from("draft");

    assert!(!chat.previous_input_history());
    assert_eq!(chat.input.value(), "draft");
}

#[test]
fn record_input_history_keeps_latest_entries() {
    let mut chat = ChatState::default();

    chat.record_input_history("first");
    chat.record_input_history("first");
    chat.record_input_history("second");

    assert_eq!(
        chat.input_history,
        vec!["first".to_string(), "second".to_string()]
    );
}

#[test]
fn enqueue_pending_turn_adds_fifo_item_without_transcript_message() {
    let mut chat = ChatState::default();
    chat.messages.clear();
    chat.input = Input::from("queued");

    chat.enqueue_pending_turn("queued".to_string(), None, 0);

    assert_eq!(chat.input.value(), "");
    assert!(chat.stick_to_bottom);
    assert!(chat.messages.is_empty());
    assert_eq!(
        chat.pop_pending_turn().map(|queued| queued.prompt),
        Some("queued".to_string())
    );
    assert!(!chat.has_pending_turns());
}

#[test]
fn synced_scrollbar_reaches_track_bottom_when_chat_is_at_bottom() {
    let mut chat = ChatState::default();
    chat.total_lines = 100;
    chat.last_visible_height = 20;
    chat.scroll_offset = chat.max_scroll_offset();
    chat.sync_scrollbar_state();

    let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 5));
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .render(buffer.area, &mut buffer, &mut chat.scrollbar_state);

    assert_eq!(buffer[(0, 4)].symbol(), "█");
}

#[test]
fn total_line_scrollbar_state_leaves_gap_at_bottom_for_chat_offsets() {
    let mut legacy_state = ScrollbarState::default()
        .content_length(100)
        .viewport_content_length(20)
        .position(80);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 5));
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("░"))
        .render(buffer.area, &mut buffer, &mut legacy_state);

    assert_eq!(buffer[(0, 4)].symbol(), "░");
}

#[test]
fn page_step_overlaps_one_line_for_pager_style_paging() {
    let mut chat = ChatState::default();
    chat.last_visible_height = 20;
    assert_eq!(chat.page_step(), 19);

    // A viewport of a single line still pages by one.
    chat.last_visible_height = 1;
    assert_eq!(chat.page_step(), 1);

    // A zero-height viewport degrades to a one-line step instead of stalling.
    chat.last_visible_height = 0;
    assert_eq!(chat.page_step(), 1);
}

#[test]
fn scroll_page_up_moves_back_by_viewport_and_unsticks_from_bottom() {
    let mut chat = ChatState::default();
    chat.total_lines = 100;
    chat.last_visible_height = 20;
    // Start glued to the streaming tail.
    chat.scroll_offset = chat.max_scroll_offset(); // 80
    chat.stick_to_bottom = true;

    chat.scroll_page_up();

    // 80 - 19 = 61, with one line of overlap retained.
    assert_eq!(chat.scroll_offset, 61);
    assert!(!chat.stick_to_bottom);
}

#[test]
fn scroll_page_up_clamps_at_top_without_overshooting() {
    let mut chat = ChatState::default();
    chat.total_lines = 100;
    chat.last_visible_height = 20;
    chat.scroll_offset = 5;
    chat.stick_to_bottom = false;

    chat.scroll_page_up();

    // 5.saturating_sub(19) == 0; never goes negative.
    assert_eq!(chat.scroll_offset, 0);
    assert!(!chat.stick_to_bottom);
}

#[test]
fn scroll_page_down_advances_by_viewport_and_resticks_at_bottom() {
    let mut chat = ChatState::default();
    chat.total_lines = 100;
    chat.last_visible_height = 20;
    chat.scroll_offset = 0;
    chat.stick_to_bottom = false;

    chat.scroll_page_down();

    // 0 + 19 = 19, one line of overlap with the previous page.
    assert_eq!(chat.scroll_offset, 19);
    assert!(!chat.stick_to_bottom);

    // Paging further eventually lands exactly on the bottom and re-sticks.
    chat.scroll_page_down(); // 19 + 19 = 38
    chat.scroll_page_down(); // 38 + 19 = 57
    chat.scroll_page_down(); // 57 + 19 = 76
    chat.scroll_page_down(); // 76 + 19 = 95, clamped to max 80
    assert_eq!(chat.scroll_offset, chat.max_scroll_offset());
    assert!(chat.stick_to_bottom);
}

#[test]
fn scroll_page_down_at_bottom_keeps_stick_to_bottom() {
    let mut chat = ChatState::default();
    chat.total_lines = 100;
    chat.last_visible_height = 20;
    chat.scroll_offset = chat.max_scroll_offset();
    chat.stick_to_bottom = true;

    chat.scroll_page_down();

    // Already at the bottom: stays at max and remains stuck.
    assert_eq!(chat.scroll_offset, chat.max_scroll_offset());
    assert!(chat.stick_to_bottom);
}

#[test]
fn page_scroll_respects_short_transcript_without_overflow() {
    let mut chat = ChatState::default();
    chat.total_lines = 5;
    chat.last_visible_height = 20;
    chat.scroll_offset = 0;

    // max_scroll_offset is 0 (everything fits), so paging down stays put and sticks.
    chat.scroll_page_down();
    assert_eq!(chat.scroll_offset, 0);
    assert!(chat.stick_to_bottom);
}

#[test]
fn messages_from_chat_messages_drops_system_role_and_maps_user_assistant_text() {
    use super::messages_from_chat_messages;
    use super::MessageRole;
    use xiaoo_api::chat::ChatMessage;
    use xiaoo_api::chat::{ContentBlock, MessageRole as LlmRole};

    let messages = vec![
        ChatMessage {
            role: LlmRole::System,
            blocks: vec![ContentBlock::Text {
                text: "you are helpful".to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        },
        ChatMessage {
            role: LlmRole::User,
            blocks: vec![ContentBlock::Text {
                text: "hello there".to_string(),
            }],
            message_id: None,
            timestamp_ms: 1,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        },
        ChatMessage {
            role: LlmRole::Assistant,
            blocks: vec![
                ContentBlock::Text {
                    text: "thinking...".to_string(),
                },
                ContentBlock::Text {
                    text: "answer".to_string(),
                },
            ],
            message_id: None,
            timestamp_ms: 2,
            api_usage_tokens: None,
            reasoning_content: Some("reasoning here".to_string()),
            estimated_tokens: None,
        },
    ];

    let tui = messages_from_chat_messages(messages);

    // System message dropped; user text joined; assistant text blocks
    // joined with thinking content preserved.
    assert_eq!(tui.len(), 2);
    assert_eq!(tui[0].role, MessageRole::User);
    assert_eq!(tui[0].content, "hello there");
    assert_eq!(tui[1].role, MessageRole::Assistant);
    assert_eq!(tui[1].content, "thinking...\nanswer");
    assert_eq!(tui[1].thinking_content, "reasoning here");
    assert!(!tui[1].is_streaming);
}

#[test]
fn messages_from_chat_messages_merges_tool_use_and_tool_result_into_one_card() {
    use super::messages_from_chat_messages;
    use super::{MessageRole, ToolExecutionStatus};
    use xiaoo_api::chat::ChatMessage;
    use xiaoo_api::chat::{ContentBlock, MessageRole as LlmRole};

    let messages = vec![
        ChatMessage {
            role: LlmRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
                input: serde_json::json!({ "command": "ls" }),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        },
        ChatMessage {
            role: LlmRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
                output: "file.txt".to_string(),
                is_error: false,
            }],
            message_id: None,
            timestamp_ms: 1,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        },
    ];

    let tui = messages_from_chat_messages(messages);

    // ToolUse + ToolResult collapse to a single tool card carrying both
    // args (from the ToolUse) and output (from the ToolResult).
    assert_eq!(tui.len(), 1);
    assert_eq!(tui[0].role, MessageRole::Tool);
    let tool = tui[0]
        .tool_state
        .as_ref()
        .expect("tool_state should be populated");
    assert_eq!(tool.call_id, "call-1");
    assert_eq!(tool.tool, "shell");
    assert!(tool.args_preview.contains("ls"));
    assert_eq!(tool.detail, "file.txt");
    assert_eq!(tool.status, ToolExecutionStatus::Completed);
}

#[test]
fn messages_from_chat_messages_marks_failed_tool_results() {
    use super::messages_from_chat_messages;
    use super::ToolExecutionStatus;
    use xiaoo_api::chat::ChatMessage;
    use xiaoo_api::chat::{ContentBlock, MessageRole as LlmRole};

    let messages = vec![ChatMessage {
        role: LlmRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            call_id: "call-9".to_string(),
            tool_name: "file_edit".to_string(),
            output: "permission denied".to_string(),
            is_error: true,
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    }];

    let tui = messages_from_chat_messages(messages);
    assert_eq!(tui.len(), 1);
    let tool = tui[0]
        .tool_state
        .as_ref()
        .expect("tool_state should be populated");
    assert_eq!(tool.status, ToolExecutionStatus::Failed);
    assert_eq!(tool.detail, "permission denied");
    // No matching ToolUse → tool_name falls back to the ToolResult's name.
    assert_eq!(tool.tool, "file_edit");
}
