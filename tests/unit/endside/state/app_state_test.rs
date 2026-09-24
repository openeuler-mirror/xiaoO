use super::*;
use super::{
    current_sandbox_id, sandbox_backend_config, sandbox_display_name, ApiKeyDialogState, AppState,
    PlanPanelState, RuntimeStatusLight, WideTableScrollRegion,
};
use crate::backend::GatewayBackendConfig;
use crate::config::{AgentRoleConfig, Config};
use crate::input::Input;
use crate::interaction_prompt::{PromptChoice, PromptRequest};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use xiaoo_api::chat::ReasoningEffort;

#[test]
fn runtime_status_light_is_idle_by_default() {
    let state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    assert_eq!(state.runtime_status_light(), RuntimeStatusLight::Idle);
}

#[test]
fn agent_tab_labels_allow_core_in_configured_order() {
    let mut config = Config::default();
    for role_id in ["baize", "xuanyuan", "plan"] {
        config
            .agent
            .insert(role_id.to_string(), AgentRoleConfig::default());
    }
    config.tui.agent_order = vec![
        "xuanyuan".to_string(),
        "Core".to_string(),
        "baize".to_string(),
    ];
    let mut state =
        AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");

    assert_eq!(
        state.agent_tab_labels(),
        vec!["xuanyuan", "Core", "baize", "plan"]
    );

    assert!(state.cycle_agent_role(false));
    assert_eq!(state.active_agent_tab_label(), "baize");
    assert!(state.cycle_agent_role(false));
    assert_eq!(state.active_agent_tab_label(), "plan");
}

#[test]
fn sandbox_backend_config_preserves_local_options_when_enabling_seatbelt() {
    let current = Some(GatewayBackendConfig::new(
        "local",
        json!({"default_shell": "/bin/zsh"}),
    ));

    let updated = sandbox_backend_config("seatbelt", &current).expect("backend");

    assert_eq!(updated.kind, "local");
    assert_eq!(updated.options["default_shell"], "/bin/zsh");
    assert_eq!(updated.options["isolation"]["kind"], "macos_seatbelt");
}

#[test]
fn sandbox_backend_config_removes_only_isolation_when_switching_local() {
    let current = Some(GatewayBackendConfig::new(
        "local",
        json!({
            "default_shell": "/bin/zsh",
            "isolation": {"kind": "macos_seatbelt"}
        }),
    ));

    let updated = sandbox_backend_config("local", &current).expect("backend");

    assert_eq!(updated.options["default_shell"], "/bin/zsh");
    assert!(updated.options.get("isolation").is_none());
}

#[test]
fn sandbox_backend_config_preserves_local_options_when_enabling_bubblewrap() {
    let current = Some(GatewayBackendConfig::new(
        "local",
        json!({"default_shell": "/bin/bash"}),
    ));

    let updated = sandbox_backend_config("bubblewrap", &current).expect("backend");

    assert_eq!(updated.kind, "local");
    assert_eq!(updated.options["default_shell"], "/bin/bash");
    assert_eq!(updated.options["isolation"]["kind"], "linux_bubblewrap");
}

#[test]
fn sandbox_backend_config_preserves_local_options_when_enabling_dyn_sandbox() {
    let current = Some(GatewayBackendConfig::new(
        "local",
        json!({"default_shell": "/bin/bash"}),
    ));

    let updated = sandbox_backend_config("dynsandbox", &current).expect("backend");

    assert_eq!(updated.kind, "local");
    assert_eq!(updated.options["default_shell"], "/bin/bash");
    assert_eq!(updated.options["isolation"]["kind"], "linux_dynsandbox");
}

#[test]
fn sandbox_helpers_recognize_bubblewrap() {
    let mut config = Config::default();
    config.operation_backend = Some(GatewayBackendConfig::new(
        "local",
        json!({"isolation": {"kind": "linux_bubblewrap"}}),
    ));

    assert_eq!(current_sandbox_id(&config), "bubblewrap");
    assert_eq!(
        sandbox_display_name(&config.operation_backend),
        "Bubblewrap"
    );
}

#[test]
fn sandbox_helpers_recognize_dyn_sandbox() {
    let mut config = Config::default();
    config.operation_backend = Some(GatewayBackendConfig::new(
        "local",
        json!({"isolation": {"kind": "linux_dynsandbox"}}),
    ));

    assert_eq!(current_sandbox_id(&config), "dynsandbox");
    assert_eq!(
        sandbox_display_name(&config.operation_backend),
        "Dyn-Sandbox"
    );
}

#[test]
fn runtime_status_light_is_running_while_loading() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state.chat_state.is_loading = true;
    assert_eq!(state.runtime_status_light(), RuntimeStatusLight::Running);
}

#[test]
fn runtime_status_light_prefers_interaction_when_prompt_is_open() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state.chat_state.is_loading = true;
    state
        .open_interaction_prompt(sample_prompt_request(), true)
        .expect("interaction prompt should open");
    assert_eq!(
        state.runtime_status_light(),
        RuntimeStatusLight::AwaitingInteraction
    );
}

#[test]
fn toggle_theme_switches_between_dark_and_light() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    let initial_is_light = state.theme.is_light();

    state.toggle_theme();
    assert_ne!(state.theme.is_light(), initial_is_light);

    state.toggle_theme();
    assert_eq!(state.theme.is_light(), initial_is_light);
}

#[test]
fn cycle_reasoning_effort_rotates_off_high_max() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");

    assert_eq!(state.reasoning_effort, ReasoningEffort::Off);
    state.cycle_reasoning_effort();
    assert_eq!(state.reasoning_effort, ReasoningEffort::High);
    state.cycle_reasoning_effort();
    assert_eq!(state.reasoning_effort, ReasoningEffort::Max);
    state.cycle_reasoning_effort();
    assert_eq!(state.reasoning_effort, ReasoningEffort::Off);
}

#[test]
fn toggle_api_key_visibility_switches_between_hidden_and_plaintext() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state.api_key_dialog = Some(ApiKeyDialogState {
        provider: "demo".to_string(),
        model: "model".to_string(),
        input: Input::default(),
        error: None,
        show_plaintext: false,
    });

    state.toggle_api_key_visibility();
    assert!(state
        .api_key_dialog
        .as_ref()
        .is_some_and(|dialog| dialog.show_plaintext));

    state.toggle_api_key_visibility();
    assert!(state
        .api_key_dialog
        .as_ref()
        .is_some_and(|dialog| !dialog.show_plaintext));
}

#[test]
fn slash_menu_reopens_for_new_prefix_after_dismiss() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state.chat_state.input = "/skills".into();

    assert!(state.slash_menu_visible());

    state.dismiss_current_slash_menu();
    assert!(!state.slash_menu_visible());

    state.chat_state.input = "/".into();
    state.note_input_changed();
    assert!(state.slash_menu_visible());
}

#[test]
fn slash_menu_reopens_when_prefix_changes_after_escape() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state.chat_state.input = "/c".into();

    assert!(state.slash_menu_visible());

    state.dismiss_current_slash_menu();
    assert!(!state.slash_menu_visible());

    state.chat_state.input = "/co".into();
    state.note_input_changed();
    assert!(state.slash_menu_visible());
}

#[test]
fn session_file_change_uses_content_baseline_for_existing_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");

    let file = workspace.join("README.md");
    fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").expect("baseline");

    let mut state = AppState::new(PathBuf::from("config.toml"), workspace)
        .expect("app state should initialize");
    state.on_tool_running("call-1", "file_edit", r#"{"file_path":"README.md"}"#);

    fs::write(&file, "one\ntwo\nTHREE\nfour\nfive\n").expect("modified");
    state.on_tool_completed("call-1", "file_edit", r#"{"file_path":"README.md"}"#, None);

    let stats = state
        .session_file_changes()
        .get("README.md")
        .expect("session stats should be tracked");
    assert_eq!(stats.additions, 1);
    assert_eq!(stats.deletions, 1);
}

fn sample_prompt_request() -> PromptRequest {
    PromptRequest {
        request_id: "demo-1".to_string(),
        title: "示例交互".to_string(),
        body: Some("请选择一个选项（可填写补充说明）。".to_string()),
        choices: vec![PromptChoice {
            id: "a".to_string(),
            label: "选项 A".to_string(),
            description: Some("快速路径".to_string()),
        }],
        allow_custom_input: true,
        custom_input_hint: None,
        multi_select: false,
        is_secret: false,
        default_index: Some(0),
    }
}

#[test]
fn plan_panel_scroll_clamps_to_content_bounds() {
    let mut plan_panel = PlanPanelState::default();
    plan_panel.total_lines = 10;
    plan_panel.last_visible_height = 4;
    assert_eq!(plan_panel.max_scroll_offset(), 6);

    for _ in 0..10 {
        plan_panel.scroll_down();
    }
    assert_eq!(plan_panel.scroll_offset, 6, "scroll down must clamp at max");

    for _ in 0..10 {
        plan_panel.scroll_up();
    }
    assert_eq!(plan_panel.scroll_offset, 0, "scroll up must clamp at 0");
}

#[test]
fn plan_panel_set_scroll_offset_clamps_and_repositions_thumb() {
    let mut plan_panel = PlanPanelState::default();
    plan_panel.total_lines = 12;
    plan_panel.last_visible_height = 4;

    plan_panel.set_scroll_offset(999);
    assert_eq!(plan_panel.scroll_offset, 8, "set_scroll_offset must clamp");

    plan_panel.set_scroll_offset(3);
    assert_eq!(plan_panel.scroll_offset, 3);
}

#[test]
fn plan_panel_reset_scroll_returns_to_top() {
    let mut plan_panel = PlanPanelState::default();
    plan_panel.total_lines = 20;
    plan_panel.last_visible_height = 5;
    plan_panel.scroll_down();
    plan_panel.scroll_down();
    assert!(plan_panel.scroll_offset > 0);
    plan_panel.scrollbar_dragging = true;

    plan_panel.reset_scroll();
    assert_eq!(plan_panel.scroll_offset, 0);
    assert!(!plan_panel.scrollbar_dragging);
}

#[test]
fn plan_panel_content_shrink_clamps_stale_offset() {
    // Mirrors the render-time clamp: content shrinks (new plan snapshot
    // or sidebar resize), so a stale offset must be re-clamped instead
    // of pointing past the end.
    let mut plan_panel = PlanPanelState::default();
    plan_panel.total_lines = 30;
    plan_panel.last_visible_height = 10;
    plan_panel.scroll_offset = 15;

    plan_panel.total_lines = 12;
    plan_panel.last_visible_height = 10;
    let max = plan_panel.max_scroll_offset();
    plan_panel.scroll_offset = plan_panel.scroll_offset.min(max);
    assert_eq!(plan_panel.scroll_offset, 2);
}

#[test]
fn plan_panel_not_scrollable_when_content_fits() {
    let mut plan_panel = PlanPanelState::default();
    plan_panel.total_lines = 3;
    plan_panel.last_visible_height = 10;
    assert_eq!(plan_panel.max_scroll_offset(), 0);
    plan_panel.scroll_down();
    assert_eq!(plan_panel.scroll_offset, 0);
}

/// Build a wide-table hit region for `message_index` with `viewport_width`
/// columns and `natural_width` total columns.
fn wide_region(
    message_index: usize,
    viewport_width: usize,
    natural_width: usize,
) -> WideTableScrollRegion {
    WideTableScrollRegion {
        message_index,
        rect: ratatui::layout::Rect::new(0, 0, viewport_width as u16, 6),
        viewport_width,
        max_offset: natural_width.saturating_sub(viewport_width),
    }
}

#[test]
fn scroll_wide_table_clamps_to_boundaries_and_marks_message_dirty() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state
        .chat_state
        .messages
        .push(crate::chat::Message::system("table message".to_string()));
    let message_index = state.chat_state.messages.len() - 1;
    let revision_before = state.chat_state.messages[message_index].render_revision;

    let region = wide_region(message_index, 20, 60); // max_offset 40

    // Scrolling left past the left edge is a no-op.
    assert!(!state.scroll_wide_table(&region, -100));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        0
    );
    assert_eq!(
        state.chat_state.messages[message_index].render_revision, revision_before,
        "no-op scroll must not dirty the message"
    );

    // Right scroll moves the window and dirties the message.
    assert!(state.scroll_wide_table(&region, 30));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        30
    );
    assert_ne!(
        state.chat_state.messages[message_index].render_revision, revision_before,
        "moved window must mark the message dirty"
    );

    // Right scroll beyond the max clamps to `max_offset`.
    let revision_after = state.chat_state.messages[message_index].render_revision;
    assert!(state.scroll_wide_table(&region, 100));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        40
    );
    assert_ne!(
        state.chat_state.messages[message_index].render_revision,
        revision_after
    );

    // Once at the right edge, further right scrolling is a no-op.
    let revision_final = state.chat_state.messages[message_index].render_revision;
    assert!(!state.scroll_wide_table(&region, 1));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        40
    );
    assert_eq!(
        state.chat_state.messages[message_index].render_revision, revision_final,
        "clamped no-op must not dirty the message"
    );
}

/// The window lives on the message, so deleting an earlier message must not
/// re-attach it to whatever ended up at the same index.
#[test]
fn wide_table_window_travels_with_its_message_across_deletion() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    // The initial chat state may already carry a system message; anchor on
    // the index of the message this test pushes.
    let owner_index = state.chat_state.messages.len();
    state
        .chat_state
        .messages
        .push(crate::chat::Message::user("first"));
    state
        .chat_state
        .messages
        .push(crate::chat::Message::system("table owner"));
    let owner_index = owner_index + 1;

    assert!(state.scroll_wide_table(&wide_region(owner_index, 20, 60), 30));
    assert_eq!(
        state.chat_state.messages[owner_index].table_horiz_offset,
        30
    );

    // Delete the earlier message: the owner shifts down, the *other*
    // message now occupies the old index.
    state.chat_state.messages.remove(owner_index - 1);

    assert_eq!(
        state.chat_state.messages[owner_index - 1].content,
        "table owner",
        "sanity: the owner moved to the deleted message's index"
    );
    assert_eq!(
        state.chat_state.messages[owner_index - 1].table_horiz_offset,
        30,
        "the window must follow its message to the new index"
    );
}

/// A window can be left on a message that is no longer rendered (deleted or
/// on another transcript); scrolling must then be a no-op rather than
/// panicking or addressing the wrong list.
#[test]
fn scroll_wide_table_on_a_missing_message_is_a_no_op() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    let missing = state.chat_state.messages.len() + 5;
    assert!(!state.scroll_wide_table(&wide_region(missing, 20, 60), 10));
}

/// Several wide tables in one message share one window; a table narrower
/// than the shared window must pan from *its own* clamped start instead of
/// jumping left on the first step.
#[test]
fn scroll_wide_table_rebases_a_shared_window_into_the_targets_range() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state
        .chat_state
        .messages
        .push(crate::chat::Message::system("two tables".to_string()));
    let message_index = state.chat_state.messages.len() - 1;

    // Wide table A pushes the shared window to 60.
    let wide = wide_region(message_index, 20, 80); // max_offset 60
    assert!(state.scroll_wide_table(&wide, 60));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        60
    );

    // Narrower table B (max_offset 20) is already showing its rightmost
    // window at 60; one step left must land at 20 - step, not 60 - step.
    let narrow = wide_region(message_index, 20, 40); // max_offset 20
    assert!(state.scroll_wide_table(&narrow, -8));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        12
    );
}

#[test]
fn scroll_active_wide_table_pans_topmost_target_with_step() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state
        .chat_state
        .messages
        .push(crate::chat::Message::system("wide table".to_string()));
    let message_index = state.chat_state.messages.len() - 1;
    // Viewport 36 -> step = max(8, 36/3) = 12.
    state.render_state.wide_table_keyboard_target = Some(wide_region(message_index, 36, 80)); // max_offset 44

    assert!(state.scroll_active_wide_table(1));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        12
    );

    assert!(state.scroll_active_wide_table(1));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        24
    );

    assert!(state.scroll_active_wide_table(-1));
    assert_eq!(
        state.chat_state.messages[message_index].table_horiz_offset,
        12
    );

    // No target -> no-op (falls through to the input box / navigation).
    state.render_state.wide_table_keyboard_target = None;
    assert!(!state.scroll_active_wide_table(1));
}

/// The keyboard target of a subagent lane must scroll the *lane's* message,
/// never the main transcript's message at the same index.
#[test]
fn scroll_active_wide_table_addresses_the_active_subagent_lane() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("app state should initialize");
    state
        .chat_state
        .messages
        .push(crate::chat::Message::system("main owner".to_string()));
    state.chat_state.ensure_subagent_lane(
        "agent-1".to_string(),
        None,
        "Title".to_string(),
        "Description".to_string(),
        "Goal".to_string(),
    );
    state
        .chat_state
        .subagent_lanes
        .get_mut("agent-1")
        .expect("lane inserted above")
        .messages
        .push(crate::chat::Message::system("lane owner".to_string()));
    assert!(state.chat_state.enter_subagent_view("agent-1"));

    state.render_state.wide_table_keyboard_target = Some(wide_region(0, 20, 60));
    assert!(state.scroll_active_wide_table(1));

    assert_eq!(
        state
            .chat_state
            .subagent_lanes
            .get("agent-1")
            .expect("lane exists")
            .messages[0]
            .table_horiz_offset,
        8,
        "lane message must receive the scroll"
    );
    assert_eq!(
        state.chat_state.messages[0].table_horiz_offset, 0,
        "main transcript message at the same index must stay untouched"
    );
}

#[test]
fn table_horiz_scroll_step_uses_the_table_viewport() {
    assert_eq!(AppState::table_horiz_scroll_step(0), 8);
    assert_eq!(AppState::table_horiz_scroll_step(21), 8);
    assert_eq!(AppState::table_horiz_scroll_step(24), 8);
    assert_eq!(AppState::table_horiz_scroll_step(36), 12);
    assert_eq!(AppState::table_horiz_scroll_step(120), 40);
}

impl super::AppState {
    pub fn new(config_path: PathBuf, workspace: PathBuf) -> Result<Self, anyhow::Error> {
        Ok(Self {
            theme: Theme::default(),
            chat_state: build_chat_state(&Config::default()),
            status_panel: build_status_panel(&Config::default()),
            input_mode: InputMode::Editing,
            should_quit: false,
            quit_via_interrupt: false,
            provider_dialog: None,
            sandbox_dialog: None,
            remote_session_dialog: None,
            session_snapshot_dialog: None,
            delete_dialog: None,
            cron_dialog: None,
            api_key_dialog: None,
            agent_config: Config::default(),
            active_agent_role: None,
            reasoning_effort: Config::default().llm.reasoning_effort,
            config_path,
            workspace: workspace.clone(),
            session_messages: Vec::new(),
            plan_state: None,
            plan_panel: PlanPanelState::default(),
            session_id: uuid::Uuid::new_v4().to_string(),
            client_id: uuid::Uuid::new_v4().to_string(),
            session_taken_over: false,
            current_snapshot_context: None,
            slash: SlashState::default(),
            file_mention: FileMentionState::default(),
            interaction_prompt: None,
            render_state: RenderState::default(),
            transcript_selection: None,
            copy_notice: None,
            copy_error_notice: None,
            external_commands: load_external_commands(),
            diff_tracker: SessionDiffTracker::new(workspace),
        })
    }
}

/// Tests for the drag-selection + scroll interplay: holding the mouse button
/// while wheel-scrolling (or dragging past the box edges) scrolls the
/// transcript and extends the selection to the content now under the
/// pointer, so one drag can select — and auto-copy on release — content
/// beyond the visible viewport instead of only what is on screen.
mod transcript_selection_scroll_tests {
    use super::super::{clamp_to_transcript_content, AppState, CachedMessageRender};
    use crate::render::transcript::build_transcript_cache;
    use crate::render::wrap_line_to_visual_lines;
    use crate::selection::TranscriptSelection;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use std::path::PathBuf;

    /// A state with a 51-line transcript rendered as ONE message block whose
    /// first logical line is the role header (headers are excluded from copy
    /// extraction — `build_transcript_cache` marks line 0 of each block via
    /// `HEADER_LINE_INDEX`) followed by 50 content lines "line-00"..
    /// "line-49". Each logical line is one visual row at width 80, so visual
    /// row == logical line index. The Messages area shows 10 visible rows at
    /// `Rect(2, 5, 40, 10)` (rows 5..=14, content columns 3..=39).
    fn state_with_line_transcript() -> (AppState, Rect) {
        let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
        let mut lines: Vec<Line<'static>> = vec![Line::from("  ▎ You  12:00:00")];
        for i in 0..50 {
            lines.push(Line::from(format!("line-{i:02}")));
        }
        let wrapped_lines: Vec<Vec<Line<'static>>> = lines
            .iter()
            .map(|l| wrap_line_to_visual_lines(l, 80))
            .collect();
        let render = CachedMessageRender {
            width: 80,
            wide_tables: Vec::new(),
            lines,
            wrapped_lines: Some(wrapped_lines),
            tool_toggle_row_offset: None,
            subagent_open_target: None,
            frozen_prefix_line_count: None,
        };
        state.render_state.transcript_cache =
            Some(build_transcript_cache(None, vec![Some(render)]));
        let area = Rect::new(2, 5, 40, 10);
        state.chat_state.total_lines = 51;
        state.chat_state.last_visible_height = 10;
        state.chat_state.scroll_offset = 0;
        state.chat_state.stick_to_bottom = false;
        state.chat_state.sync_scrollbar_state();
        (state, area)
    }

    /// `transcript_drag_active` is true only while a selection exists and the
    /// scrollbar is not being dragged — the state in which wheel scrolls and
    /// edge drags extend the selection instead of dismissing it.
    #[test]
    fn transcript_drag_active_reflects_selection_and_scrollbar() {
        let (mut state, _area) = state_with_line_transcript();
        assert!(!state.transcript_drag_active(), "no selection → no drag");

        state.transcript_selection = Some(TranscriptSelection::new(0, 0));
        assert!(state.transcript_drag_active(), "selection → drag in progress");

        state.chat_state.scrollbar_dragging = true;
        assert!(
            !state.transcript_drag_active(),
            "scrollbar drag takes precedence"
        );

        state.chat_state.scrollbar_dragging = false;
        assert!(state.transcript_drag_active());
    }

    /// Core feature: holding the mouse button and rolling the wheel scrolls
    /// the content under the stationary pointer while the selection follows,
    /// so one drag can select (and on release copy) content beyond the
    /// visible viewport instead of only what is on screen.
    #[test]
    fn wheel_scroll_extends_selection_beyond_viewport() {
        let (mut state, area) = state_with_line_transcript();
        // Anchor at the start of "line-00" (logical line 1; line 0 is the
        // role header).
        state.transcript_selection = Some(TranscriptSelection::new(1, 0));
        assert!(state.transcript_drag_active());

        // Roll the wheel down three notches with the pointer parked at the
        // bottom visible row (row 14 = area.y + 9): each notch scrolls one
        // line and the selection extends to the content now under it.
        for _ in 0..3 {
            state.active_transcript_scroll_down();
            state.extend_transcript_selection_to(12, 14, area);
        }
        assert_eq!(state.chat_state.scroll_offset, 3);
        let sel = state.transcript_selection.as_ref().unwrap();
        assert_eq!(sel.anchor_line, 1, "anchor stays where the drag began");
        assert_eq!(
            sel.cursor_line, 12,
            "cursor follows the scrolled content (visual row = offset 3 + rel 9)"
        );

        // The extracted selection spans logical lines 1..=12 ("line-00"
        // through "line-11") — the anchor line is scrolled far above the
        // 10-row viewport — proving copy is not limited to visible content.
        let text = state.transcript_selected_text().expect("selection text");
        assert_eq!(text.lines().count(), 12);
        assert!(text.starts_with("line-00"));
        assert!(text.ends_with("line-11"));
    }

    /// The wheel works in the other direction too: rolling up while dragging
    /// extends the selection towards earlier content.
    #[test]
    fn wheel_scroll_up_extends_selection_towards_earlier_lines() {
        let (mut state, area) = state_with_line_transcript();
        state.chat_state.scroll_offset = 10;
        state.chat_state.sync_scrollbar_state();
        // Anchor on the first content line visible at offset 10 (visual row
        // 10 is the header, row 11 is "line-10").
        state.transcript_selection = Some(TranscriptSelection::new(11, 0));
        assert!(state.transcript_drag_active());

        // Roll the wheel up two notches with the pointer at the top visible
        // row (row 5 = area.y): offset 10 → 8, pointer maps to visual row 8.
        for _ in 0..2 {
            state.active_transcript_scroll_up();
            state.extend_transcript_selection_to(12, 5, area);
        }
        assert_eq!(state.chat_state.scroll_offset, 8);
        let sel = state.transcript_selection.as_ref().unwrap();
        assert_eq!(sel.cursor_line, 8);
        let (start_line, _, end_line, _) = sel.normalised();
        assert_eq!((start_line, end_line), (8, 11));
    }

    /// Dragging past the bottom edge (e.g. into the input box) auto-scrolls
    /// down one line per drag event and extends the selection to the last
    /// visible line.
    #[test]
    fn drag_past_bottom_edge_extends_to_last_visible_line() {
        let (mut state, area) = state_with_line_transcript();
        state.transcript_selection = Some(TranscriptSelection::new(1, 0));
        assert!(state.transcript_drag_active());

        // Pointer dragged far below the Messages box (row 40 > bottom row 14).
        state.active_transcript_scroll_down();
        state.extend_transcript_selection_to(12, 40, area);

        let sel = state.transcript_selection.as_ref().unwrap();
        assert_eq!(
            sel.cursor_line, 10,
            "row 40 clamps to the last visible row (visual = offset 1 + rel 9)"
        );
        let (start_line, _, end_line, _) = sel.normalised();
        assert_eq!((start_line, end_line), (1, 10));
    }

    /// Dragging above the top edge auto-scrolls up and extends the selection
    /// to the first visible line.
    #[test]
    fn drag_above_top_edge_extends_to_first_visible_line() {
        let (mut state, area) = state_with_line_transcript();
        state.chat_state.scroll_offset = 10;
        state.chat_state.sync_scrollbar_state();
        // Anchor near the bottom of the visible window (offset 10 → visual
        // rows 10..=19 visible; row 19 is "line-18").
        state.transcript_selection = Some(TranscriptSelection::new(19, 0));
        assert!(state.transcript_drag_active());

        // Pointer dragged above the Messages box (row 3 < top row 5).
        state.active_transcript_scroll_up();
        state.extend_transcript_selection_to(12, 3, area);

        let sel = state.transcript_selection.as_ref().unwrap();
        assert_eq!(
            sel.cursor_line, 9,
            "row 3 clamps to the first visible row (visual = offset 9 + rel 0)"
        );
        let (start_line, _, end_line, _) = sel.normalised();
        assert_eq!((start_line, end_line), (9, 19));
    }

    /// Positions from beyond the box edges clamp into the Messages content
    /// area (text starts one column after the left border and ends before
    /// the scrollbar track; rows are bounded by the box).
    #[test]
    fn clamp_to_transcript_content_bounds_positions() {
        let area = Rect::new(2, 5, 40, 10); // rows 5..=14, content cols 3..=39
        assert_eq!(clamp_to_transcript_content(10, 10, area), (10, 10));
        assert_eq!(
            clamp_to_transcript_content(0, 0, area),
            (3, 5),
            "above/left clamps to the top-left content cell"
        );
        assert_eq!(
            clamp_to_transcript_content(200, 200, area),
            (39, 14),
            "below/right clamps to the bottom-right content cell"
        );
    }

    /// Degenerate boxes (too narrow for border + scrollbar) must not panic.
    #[test]
    fn clamp_to_transcript_content_handles_narrow_area() {
        let area = Rect::new(0, 0, 3, 5);
        assert_eq!(clamp_to_transcript_content(9, 9, area), (1, 4));
        assert_eq!(clamp_to_transcript_content(0, 0, area), (1, 0));
    }
}

/// Tests for `report_clipboard_result`: the success toast must only appear
/// when delivery is confirmed (native clipboard write or an OSC 52-capable
/// terminal), while any completed attempt (`Ok`) still tells callers they
/// may clear the source selection.
mod clipboard_report_tests {
    use super::super::AppState;
    use crate::services::provider::ClipboardOutcome;
    use anyhow::anyhow;
    use std::path::PathBuf;

    fn fresh_state() -> AppState {
        AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize")
    }

    /// A native clipboard write is confirmed delivery: success toast, and
    /// `true` so the caller may clear the selection.
    #[test]
    fn native_delivery_shows_success_toast() {
        let mut state = fresh_state();
        assert!(!state.copy_notice_active());
        assert!(!state.copy_error_notice_active());
        assert!(state.report_clipboard_result(Ok(ClipboardOutcome::Native)));
        assert!(state.copy_notice_active());
        assert!(!state.copy_error_notice_active());
    }

    /// A failed copy shows the error toast and reports `false` — the caller
    /// keeps the selection so the user can retry.
    #[test]
    fn failed_copy_shows_error_toast_and_keeps_selection() {
        let mut state = fresh_state();
        assert!(!state.report_clipboard_result(Err(anyhow!("clipboard unavailable"))));
        assert!(state.copy_error_notice_active());
        assert!(!state.copy_notice_active());
    }

    /// An OSC 52 sequence is fire-and-forget: exactly one toast is shown —
    /// success when the current terminal is known to honour the sequence,
    /// the error toast otherwise — and the attempt is still reported so the
    /// selection may be cleared.
    #[test]
    fn osc52_outcome_shows_exactly_one_toast() {
        let mut state = fresh_state();
        assert!(state.report_clipboard_result(Ok(ClipboardOutcome::Osc52)));
        assert!(
            state.copy_notice_active() ^ state.copy_error_notice_active(),
            "exactly one of the copy/copy-error toasts must be active"
        );
    }
}
