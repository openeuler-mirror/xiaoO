use super::{
    default_provider_list, ChatState, Input, Message, ModelInfo, ModelListState, ProviderInfo,
    MODEL_LIST_FAILURE_BACKOFF, MODEL_LIST_TTL,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget},
};
use std::time::{Duration, Instant};

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
    let mut chat = ChatState {
        last_visible_height: 20,
        ..Default::default()
    };
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
    let mut chat = ChatState {
        total_lines: 100,
        last_visible_height: 20,
        scroll_offset: 5,
        stick_to_bottom: false,
        ..Default::default()
    };

    chat.scroll_page_up();

    // 5.saturating_sub(19) == 0; never goes negative.
    assert_eq!(chat.scroll_offset, 0);
    assert!(!chat.stick_to_bottom);
}

#[test]
fn scroll_page_down_advances_by_viewport_and_resticks_at_bottom() {
    let mut chat = ChatState {
        total_lines: 100,
        last_visible_height: 20,
        scroll_offset: 0,
        stick_to_bottom: false,
        ..Default::default()
    };

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
    let mut chat = ChatState {
        total_lines: 5,
        last_visible_height: 20,
        scroll_offset: 0,
        ..Default::default()
    };

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

// ── Official model-list state ──────────────────────────────────────────────

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
    }
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

/// `needs_refresh` is the sole on-demand-refresh criterion (`Ctrl+R`'s
/// forced refresh bypasses it), so this test pins all five states —
/// especially `Unsupported` never retrying and the `Failed` backoff window,
/// which decide whether a broken endpoint gets hammered.
#[test]
fn model_list_state_needs_refresh_follows_ttl_and_failure_backoff() {
    let now = Instant::now();

    assert!(
        ModelListState::Builtin.needs_refresh(now),
        "从未成功拉取过 → 要拉"
    );
    assert!(
        !ModelListState::Loading {
            since: Instant::now()
        }
        .needs_refresh(now),
        "已有请求在飞 → 不重复发起"
    );
    assert!(
        !ModelListState::Unsupported.needs_refresh(now),
        "profile 静态声明没有官方接口 → 永不重试"
    );

    let just_fetched = ModelListState::Ready { fetched_at: now };
    assert!(!just_fetched.needs_refresh(now), "TTL 内直接复用缓存");
    assert!(!just_fetched.needs_refresh(now + MODEL_LIST_TTL - Duration::from_secs(1)));
    assert!(
        just_fetched.needs_refresh(now + MODEL_LIST_TTL),
        "TTL 到点重拉"
    );

    let failed = ModelListState::Failed {
        message: "boom".to_string(),
        failed_at: now,
        hint_only: false,
    };
    assert!(
        !failed.needs_refresh(now + MODEL_LIST_FAILURE_BACKOFF - Duration::from_secs(1)),
        "退避窗口内不重试"
    );
    assert!(failed.needs_refresh(now + MODEL_LIST_FAILURE_BACKOFF));

    // A recorded instant later than `now` (theoretically impossible) is
    // treated as 0 rather than panicking.
    let future = ModelListState::Ready {
        fetched_at: now + Duration::from_secs(60),
    };
    assert!(!future.needs_refresh(now));
}

#[test]
fn model_list_state_defaults_to_builtin_and_tracks_loading_staleness() {
    let mut state = ChatState::new();
    let started = Instant::now();

    assert!(matches!(
        state.model_list_state("openai"),
        ModelListState::Builtin
    ));
    assert!(
        state.model_list_state("openai").needs_refresh(started),
        "从未成功 → 要拉"
    );

    state.set_model_list_state("openai", ModelListState::Loading { since: started });
    assert!(state.model_list_state("openai").is_loading());
    assert!(
        !state.model_list_state("openai").needs_refresh(started),
        "在飞不重复拉"
    );
    assert!(!state.model_list_state("openai").is_stale_loading(started));

    // Stale backstop: if the task vanishes before reporting, `Loading` must
    // become re-fetchable once the backstop window expires — inside it,
    // `needs_refresh(Loading)` is false and the in-flight slot stays taken,
    // so nothing can fetch. A session rebuild (`/new`) is no escape hatch:
    // it resets the state to `Builtin` (making `is_stale_loading` always
    // false) without clearing `model_fetch_inflight`, which would turn
    // "self-heals after 60s" into "only a process restart helps".
    let later = started + crate::chat::MODEL_LIST_STALE_AFTER + Duration::from_secs(1);
    assert!(state.model_list_state("openai").is_stale_loading(later));
    assert!(
        state.model_list_state("openai").needs_refresh(later),
        "陈旧 Loading 必须可重拉"
    );

    state.set_model_list_state("kimi", ModelListState::Unsupported);
    assert!(
        !state.model_list_state("kimi").needs_refresh(later),
        "无官方 catalog 的 provider 永不重试"
    );
}

#[test]
fn replace_provider_models_replaces_only_the_named_provider() {
    let mut state = ChatState::new();
    state.available_providers = vec![
        ProviderInfo {
            name: "openai".to_string(),
            models: vec![model("gpt")],
        },
        ProviderInfo {
            name: "deepseek".to_string(),
            models: vec![model("deepseek-chat")],
        },
    ];

    assert!(state.replace_provider_models("deepseek", vec![model("a"), model("b")]));

    assert_eq!(
        ids(&state.available_providers[0].models),
        vec!["gpt"],
        "其他 provider 的列表不得被牵连"
    );
    assert_eq!(ids(&state.available_providers[1].models), vec!["a", "b"]);
}

#[test]
fn replace_provider_models_returns_false_for_unknown_provider() {
    let mut state = ChatState::new();
    let before: Vec<String> = state.available_providers[0]
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect();

    assert!(!state.replace_provider_models("no-such-provider", vec![model("x")]));

    let after: Vec<String> = state.available_providers[0]
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect();
    assert_eq!(
        after, before,
        "未命中时不得改动任何 provider（失败路径不写列表由此保证）"
    );
}

#[test]
fn history_preserves_multiline_controls_and_restores_draft() {
    let mut chat = ChatState::default();
    let entry = "\t中\r\n文\t ";
    let draft = "A\tB\r";
    chat.set_input_history(vec![entry.to_string()]);
    chat.input = Input::from(draft);
    for _ in 0..2 {
        assert!(chat.previous_input_history());
        assert_eq!(chat.input.value().as_bytes(), entry.as_bytes());
        assert!(chat.next_input_history());
        assert_eq!(chat.input.value().as_bytes(), draft.as_bytes());
    }
}
