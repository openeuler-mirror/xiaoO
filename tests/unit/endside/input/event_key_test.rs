use super::*;
use std::time::{Duration, Instant};
use crate::config::Config;
use crate::input::Input;
use crate::services::command_loader::ExternalCommand;
use crossterm::event::KeyModifiers;
use std::path::PathBuf;

fn external_commands() -> Vec<ExternalCommand> {
    vec![ExternalCommand {
        name: "review".to_string(),
        description: "Review code".to_string(),
        body: "Review this carefully.".to_string(),
    }]
}

// ── hard-coded app-level shortcuts (readline / emacs) ──────────────────────

#[test]
fn submit_chord_is_enter_and_shift_enter() {
    assert!(is_submit_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::empty()
    )));
    // Terminals that can report modifiers deliver Shift+Enter as Enter+SHIFT.
    assert!(is_submit_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::SHIFT
    )));
    // Alt+Enter / Ctrl+Enter are newlines, not submits.
    assert!(!is_submit_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::ALT
    )));
    assert!(!is_submit_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::CONTROL
    )));
}

#[test]
fn insert_newline_chord_covers_ctrl_j_alt_enter_and_ctrl_enter() {
    assert!(is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Char('j'),
        event::KeyModifiers::CONTROL
    )));
    // Uppercase J is the same physical key.
    assert!(is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Char('J'),
        event::KeyModifiers::CONTROL
    )));
    assert!(is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::ALT
    )));
    assert!(is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::CONTROL
    )));
    // Plain Enter submits; plain J / Alt+J are not newlines.
    assert!(!is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Enter,
        event::KeyModifiers::empty()
    )));
    assert!(!is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Char('j'),
        event::KeyModifiers::empty()
    )));
    assert!(!is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Char('j'),
        event::KeyModifiers::ALT
    )));
}

#[test]
fn ctrl_shift_j_is_not_a_newline_chord() {
    // Modifier bits are compared exactly, so Ctrl+Shift+J is never hijacked.
    assert!(!is_insert_newline_chord(&KeyEvent::new(
        KeyCode::Char('j'),
        event::KeyModifiers::CONTROL | event::KeyModifiers::SHIFT
    )));
}

#[test]
fn history_chords_are_ctrl_p_and_ctrl_n() {
    assert!(is_history_previous_chord(&KeyEvent::new(
        KeyCode::Char('p'),
        event::KeyModifiers::CONTROL
    )));
    assert!(is_history_next_chord(&KeyEvent::new(
        KeyCode::Char('n'),
        event::KeyModifiers::CONTROL
    )));
    assert!(!is_history_previous_chord(&KeyEvent::new(
        KeyCode::Char('p'),
        event::KeyModifiers::empty()
    )));
    assert!(!is_history_next_chord(&KeyEvent::new(
        KeyCode::Char('n'),
        event::KeyModifiers::ALT
    )));
}

#[test]
fn copy_and_cut_chords() {
    assert!(is_copy_chord(&KeyEvent::new(
        KeyCode::Insert,
        event::KeyModifiers::CONTROL
    )));
    assert!(is_copy_chord(&KeyEvent::new(
        KeyCode::Insert,
        event::KeyModifiers::CONTROL | event::KeyModifiers::SHIFT
    )));
    assert!(!is_copy_chord(&KeyEvent::new(
        KeyCode::Insert,
        event::KeyModifiers::empty()
    )));
    assert!(is_cut_chord(&KeyEvent::new(
        KeyCode::Char('x'),
        event::KeyModifiers::CONTROL
    )));
    assert!(!is_cut_chord(&KeyEvent::new(
        KeyCode::Char('x'),
        event::KeyModifiers::empty()
    )));
}

#[test]
fn plain_left_leaves_subagent_view() {
    assert!(is_leave_subagent_view_key(&KeyEvent::new(
        KeyCode::Left,
        event::KeyModifiers::empty()
    )));
}

#[test]
fn modified_left_does_not_leave_subagent_view() {
    assert!(!is_leave_subagent_view_key(&KeyEvent::new(
        KeyCode::Left,
        event::KeyModifiers::SHIFT
    )));
}

#[test]
fn former_shift_up_shortcut_does_not_leave_subagent_view() {
    assert!(!is_leave_subagent_view_key(&KeyEvent::new(
        KeyCode::Up,
        event::KeyModifiers::SHIFT
    )));
}

#[test]
fn alt_left_and_right_pan_the_wide_table() {
    assert_eq!(
        wide_table_pan_direction(&KeyEvent::new(KeyCode::Left, event::KeyModifiers::ALT)),
        Some(-1)
    );
    assert_eq!(
        wide_table_pan_direction(&KeyEvent::new(KeyCode::Right, event::KeyModifiers::ALT)),
        Some(1)
    );
}

#[test]
fn unmodified_arrows_never_pan_the_wide_table() {
    // Bare Left/Right must stay on the caret and on "leave subagent view":
    // they are the primary editing keys and must not depend on whether a
    // wide table happens to be on screen.
    for modifiers in [
        event::KeyModifiers::empty(),
        event::KeyModifiers::SHIFT,
        event::KeyModifiers::CONTROL,
        event::KeyModifiers::ALT | event::KeyModifiers::SHIFT,
    ] {
        assert_eq!(
            wide_table_pan_direction(&KeyEvent::new(KeyCode::Left, modifiers)),
            None,
            "modifiers {modifiers:?} must not pan"
        );
        assert_eq!(
            wide_table_pan_direction(&KeyEvent::new(KeyCode::Right, modifiers)),
            None,
            "modifiers {modifiers:?} must not pan"
        );
    }
}

#[test]
fn alt_arrows_on_unrelated_keys_do_not_pan() {
    assert_eq!(
        wide_table_pan_direction(&KeyEvent::new(KeyCode::Up, event::KeyModifiers::ALT)),
        None
    );
    assert_eq!(
        wide_table_pan_direction(&KeyEvent::new(KeyCode::Char('h'), event::KeyModifiers::ALT)),
        None
    );
}

#[test]
fn external_command_exact_match_expands_to_body() {
    assert_eq!(
        expand_external_command("/review", &external_commands()),
        Some((
            "Review this carefully.".to_string(),
            xiaoo_api::chat::CommandContext {
                command: "review".to_string(),
                arguments: "".to_string()
            }
        ))
    );
}

#[test]
fn external_command_appends_user_input_after_command_token() {
    assert_eq!(
        expand_external_command("/review src/main.rs 看一下边界条件", &external_commands()),
        Some((
            "Review this carefully.\n\nsrc/main.rs 看一下边界条件".to_string(),
            xiaoo_api::chat::CommandContext {
                command: "review".to_string(),
                arguments: "src/main.rs 看一下边界条件".to_string()
            }
        ))
    );
}

#[test]
fn external_command_match_only_uses_first_token() {
    assert_eq!(
        expand_external_command("/review-extra input", &external_commands()),
        None
    );
    assert_eq!(
        expand_external_command("/reviewer input", &external_commands()),
        None
    );
}

#[test]
fn external_command_with_empty_body_uses_user_input() {
    let commands = vec![ExternalCommand {
        name: "ask".to_string(),
        description: String::new(),
        body: String::new(),
    }];
    assert_eq!(
        expand_external_command("/ask hello", &commands),
        Some((
            "hello".to_string(),
            xiaoo_api::chat::CommandContext {
                command: "ask".to_string(),
                arguments: "hello".to_string()
            }
        ))
    );
}

// ── app-level integration through the real dispatch ────────────────────────
// Lock in the hard-coded Ctrl+X cut, Ctrl+Insert copy, Ctrl+P/N history,
// Ctrl+J / Alt+Enter / Ctrl+Enter newline and Enter / Shift+Enter submit
// *through the real dispatch* — `App::handle_key_event`, which resolves
// copy/cut at the top and the remaining app-level chords inside
// `handle_editing_mode_key` — not merely the Input helpers, so the tests are
// deterministic on any developer machine. The clipboard may be absent in CI,
// so copy/cut feedback is asserted as "attempted" (either the success or the
// error notice) and the deterministic line-state is what is checked precisely.

fn test_app() -> App {
    let config = Config::default();
    App::new_with_config(
        &config,
        PathBuf::from("/nonexistent/endside-test-config.toml"),
        PathBuf::from("/tmp/endside-test-workspace"),
    )
    .expect("App::new_with_config must build a headless test app")
}

fn select_text(input: &mut Input, start: usize, end: usize) {
    input.set_cursor(start);
    input.set_anchor();
    input.set_cursor(end);
}

fn copy_feedback(app: &App) -> bool {
    app.state.copy_notice.is_some() || app.state.copy_error_notice.is_some()
}

/// Force `copy_to_clipboard` onto its deterministic OSC-52 fallback (writes an
/// escape sequence to this test's captured stdout and returns `Ok`) instead of
/// shelling out to `wl-copy` / `xclip` / `xsel` / arboard bound to the host's
/// live display.
///
/// The dev machines these tests run on often have `DISPLAY` / `WAYLAND_DISPLAY`
/// set (e.g. a VNC/Xvfb) with no *working* clipboard daemon behind them; the
/// real `copy_to_clipboard` then blocks synchronously in `wl-copy` / `arboard`
/// for minutes (it also hangs the TUI event loop!). The vars are removed
/// process-wide and deliberately NOT restored: the test binary exits right
/// after, and concurrent removal from several tests is idempotent.
fn force_deterministic_clipboard_path() {
    // Edition 2021: std::env mutation is not yet `unsafe`.
    std::env::remove_var("WAYLAND_DISPLAY");
    std::env::remove_var("DISPLAY");
    std::env::remove_var("TMUX");
}

#[tokio::test]
async fn ctrl_x_cuts_selected_text_through_real_dispatch() {
    force_deterministic_clipboard_path();
    let mut app = test_app();
    let mut input = Input::from("hello world");
    select_text(&mut input, 6, 11); // "world"
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))
        .await
        .expect("dispatch Ctrl+X");

    // The cut ran — selection removed, caret back at the range start
    // (independent of whether the headless clipboard actually worked)…
    assert_eq!(app.state.chat_state.input.value(), "hello ");
    assert_eq!(app.state.chat_state.input.cursor(), 6);
    assert!(app.state.chat_state.input.selected_text().is_none());
    // …the clipboard was attempted, and Ctrl+X never quit the app:
    assert!(copy_feedback(&app), "cut must report clipboard feedback");
    assert!(!app.state.should_quit);
}

#[tokio::test]
async fn ctrl_x_without_selection_is_a_noop_through_dispatch() {
    let mut app = test_app();
    let mut input = Input::from("hello");
    input.set_cursor(3);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))
        .await
        .expect("dispatch Ctrl+X");

    assert_eq!(app.state.chat_state.input.value(), "hello");
    assert_eq!(app.state.chat_state.input.cursor(), 3);
    assert!(!copy_feedback(&app), "no selection ⇒ no cut/copy attempt");
    assert!(!app.state.should_quit);
}

#[tokio::test]
async fn ctrl_insert_copies_selection_through_real_dispatch() {
    force_deterministic_clipboard_path();
    let mut app = test_app();
    let mut input = Input::from("hello world");
    select_text(&mut input, 6, 11);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Insert, KeyModifiers::CONTROL))
        .await
        .expect("dispatch Ctrl+Insert");

    // Copy never mutates the line…
    assert_eq!(app.state.chat_state.input.value(), "hello world");
    // …the copy path actually ran (not swallowed by another action):
    assert!(
        copy_feedback(&app),
        "Ctrl+Insert must route to the copy path"
    );
    assert!(!app.state.should_quit);
}

#[tokio::test]
async fn ctrl_shift_insert_copies_selection_too() {
    force_deterministic_clipboard_path();
    let mut app = test_app();
    let mut input = Input::from("hello world");
    select_text(&mut input, 6, 11);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(
        KeyCode::Insert,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
    .await
    .expect("dispatch Ctrl+Shift+Insert");

    assert_eq!(app.state.chat_state.input.value(), "hello world");
    assert!(copy_feedback(&app), "Ctrl+Shift+Insert must route to copy");
    assert!(!app.state.should_quit);
}

#[tokio::test]
async fn ctrl_j_inserts_newline_through_real_dispatch() {
    let mut app = test_app();
    let mut input = Input::from("hello world");
    input.set_cursor(6);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL))
        .await
        .expect("dispatch Ctrl+J");

    assert_eq!(app.state.chat_state.input.value(), "hello \nworld");
    assert_eq!(app.state.chat_state.input.cursor(), 7);
    // A newline is not a submit: still editing, nothing sent.
    assert!(app.state.input_mode == InputMode::Editing);
}

#[tokio::test]
async fn alt_enter_inserts_newline_through_real_dispatch() {
    let mut app = test_app();
    let mut input = Input::from("hi");
    input.set_cursor(2);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT))
        .await
        .expect("dispatch Alt+Enter");

    assert_eq!(app.state.chat_state.input.value(), "hi\n");
    assert_eq!(app.state.chat_state.input.cursor(), 3);
    assert!(app.state.input_mode == InputMode::Editing);
}

#[tokio::test]
async fn ctrl_enter_inserts_newline_through_real_dispatch() {
    let mut app = test_app();
    let mut input = Input::from("hi");
    input.set_cursor(2);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .await
        .expect("dispatch Ctrl+Enter");

    assert_eq!(app.state.chat_state.input.value(), "hi\n");
    assert_eq!(app.state.chat_state.input.cursor(), 3);
    assert!(app.state.input_mode == InputMode::Editing);
}

#[tokio::test]
async fn ctrl_shift_j_does_not_insert_newline_through_real_dispatch() {
    // `is_insert_newline_chord` compares the modifier bits *exactly*, so
    // Ctrl+Shift+J must never be hijacked as a newline (and, being a
    // Ctrl+letter chord, must not type either).
    let mut app = test_app();
    let mut input = Input::from("hi");
    input.set_cursor(2);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('J'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
    .await
    .expect("dispatch Ctrl+Shift+J");

    assert_eq!(app.state.chat_state.input.value(), "hi");
    assert_eq!(app.state.chat_state.input.cursor(), 2);
    assert!(app.state.input_mode == InputMode::Editing);
}

#[tokio::test]
async fn shift_enter_submits_through_real_dispatch() {
    // Terminals that CAN report modifiers (kitty keyboard / CSI u, delivered
    // even without the app opting in) send Shift+Enter as a distinct
    // `Enter`+SHIFT event. `is_submit_chord` accepts both `Enter`+NONE and
    // `Enter`+SHIFT, so Shift+Enter must go through the real dispatch and
    // actually submit instead of silently no-op'ing.
    //
    // `submit_editing_input` records the line into input history
    // (~/.xiaoo/input_history.json) before anything else, so isolate HOME into
    // a fresh temp dir: the baseline starts empty and the test never reads or
    // writes the developer's real history (same spirit as
    // `force_deterministic_clipboard_path` below).
    let temp_home = std::env::temp_dir().join("xiaoo-endside-shift-enter-test-home");
    let _ = std::fs::remove_dir_all(&temp_home);
    std::fs::create_dir_all(&temp_home).expect("create temp HOME for input-history isolation");
    std::env::set_var("HOME", &temp_home);

    let mut app = test_app();
    let mut input = Input::from("hello");
    input.set_cursor(5);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
        .await
        .expect("dispatch Shift+Enter");

    // Recording the submitted line into input history is the reliable
    // "submit actually ran" signal — a no-op regression would have left the
    // history empty.
    assert_eq!(
        app.state.chat_state.input_history,
        vec!["hello".to_string()],
        "Shift+Enter must submit (not silently no-op)"
    );
    assert!(!app.state.should_quit);
}

#[tokio::test]
async fn ctrl_p_recalls_previous_history_through_real_dispatch() {
    let mut app = test_app();
    app.state.chat_state.input_history = vec!["older".to_string(), "newest".to_string()];
    app.state.chat_state.input = Input::from("draft");

    app.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
        .await
        .expect("dispatch Ctrl+P");

    assert_eq!(app.state.chat_state.input.value(), "newest");
}

#[tokio::test]
async fn unbound_alt_letter_does_not_type_through_real_dispatch() {
    // The unbound Alt+letter drop lives in a single place — `handle_input_key`
    // (the main-flow edit arm used to repeat it and early-return). Alt+Q (never
    // bound) must not type the bare letter; a bound Alt chord on the same path
    // still applies.
    let mut app = test_app();
    let mut input = Input::from("hi");
    input.set_cursor(2);
    app.state.chat_state.input = input;

    app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT))
        .await
        .expect("dispatch Alt+Q");

    assert_eq!(
        app.state.chat_state.input.value(),
        "hi",
        "unbound Alt+letter must not type the bare letter"
    );
    assert_eq!(app.state.chat_state.input.cursor(), 2);
    assert!(!app.state.should_quit);

    // Bound Alt chord on the same path is still applied one layer down
    // (Alt+F = word-right): caret moves past the only word.
    input = Input::from("hi");
    input.set_cursor(0);
    app.state.chat_state.input = input;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT))
        .await
        .expect("dispatch Alt+F");
    assert_eq!(app.state.chat_state.input.value(), "hi");
    assert_eq!(
        app.state.chat_state.input.cursor(),
        2,
        "bound Alt+F must still move by word through handle_input_key"
    );
}

// ── 官方 model list 拉取的触发与落地 ────────────────────────────────────────
//
// 这些用例走真实的按键派发与真实的 HTTP 客户端：mockito 起本地假 `/models`
// 端点，把 `llm.api_base` 指过去。拉取任务用 `recv().await` 确定性驱动，
// 不靠 sleep 轮询；只断言"同步写入的状态"的用例则完全不等待网络。

fn model(id: &str) -> crate::chat::ModelInfo {
    crate::chat::ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
    }
}

fn model_ids(models: &[crate::chat::ModelInfo]) -> Vec<String> {
    models.iter().map(|model| model.id.clone()).collect()
}

fn model_list_state(app: &App, provider: &str) -> crate::chat::ModelListState {
    app.state.chat_state.model_list_state(provider)
}

/// 把 `llm` 指向本地假端点：provider 与 `llm.provider` 一致时
/// `persisted_selection_settings` 才会用配置里的 api_base（否则回落默认表，
/// 请求会打到真实厂商）。模型 id 特意与官方列表不同，用来验证"配置模型补首位"。
fn test_app_with_llm(provider: &str, model: &str, api_base: &str) -> App {
    let mut config = Config::default();
    config.llm.provider = provider.to_string();
    config.llm.model = model.to_string();
    config.llm.api_base = api_base.to_string();
    App::new_with_config(
        &config,
        PathBuf::from("/nonexistent/endside-test-config.toml"),
        PathBuf::from("/tmp/endside-test-workspace"),
    )
    .expect("App::new_with_config must build a headless test app")
}

fn ready_outcome(provider: &str, ids: &[&str]) -> crate::provider_service::ProviderModelFetchOutcome {
    crate::provider_service::ProviderModelFetchOutcome {
        provider: provider.to_string(),
        result: Ok(ids.iter().map(|id| model(id)).collect()),
    }
}

fn failed_outcome(provider: &str, message: &str) -> crate::provider_service::ProviderModelFetchOutcome {
    crate::provider_service::ProviderModelFetchOutcome {
        provider: provider.to_string(),
        result: Err(crate::provider_service::ModelFetchError::Failed(
            message.to_string(),
        )),
    }
}

/// `/connect` 打开弹窗后必须为当前高亮 provider 起一次拉取（从未拉过 →
/// `Builtin` → 要拉），并且这一步是同步置位的，UI 立刻能显示 "refreshing…"。
#[tokio::test]
async fn opening_connect_schedules_fetch_for_active_provider() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body(r#"{"data":[{"id":"m1"}]}"#)
        .create_async()
        .await;
    let mut app = test_app_with_llm("openai-compatible", "pin-me", &format!("{}/v1", server.url()));

    app.open_provider_selection_dialog();

    assert_eq!(
        app.state
            .provider_dialog
            .as_ref()
            .and_then(|dialog| dialog.selected_provider_name()),
        Some("openai-compatible"),
        "配置里的 provider 会被 merge_config_provider 追加进弹窗并预选"
    );
    assert!(
        model_list_state(&app, "openai-compatible").is_loading(),
        "打开弹窗应立即进入 Loading"
    );
}

/// 端到端：真实 HTTP 响应 → 结果回传 → 落地。官方列表替换弹窗与 ChatState
/// 两处副本、配置模型补首位、选中项按 id 保持、状态转 Ready。
///
/// provider 用 `local`：它的 profile 不要求 API key（`api_key_required=false`），
/// 因此请求能真正打到假端点，不需要在测试里摆弄环境变量。
#[tokio::test]
async fn fetched_official_list_replaces_dialog_models_end_to_end() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"id":"official-a"},{"id":"official-b"}]}"#)
        .create_async()
        .await;
    let mut app = test_app_with_llm("local", "pin-me", &format!("{}/v1", server.url()));

    app.open_provider_selection_dialog();
    let (session, outcome) = app
        .model_fetch_rx
        .recv()
        .await
        .expect("拉取任务必定回传结果（join_fetch_outcome 保证 panic 也折成结果）");
    app.apply_model_fetch_outcome(session, outcome);

    mock.assert_async().await;
    let dialog = app
        .state
        .provider_dialog
        .as_ref()
        .expect("弹窗仍然打开");
    let dialog_ids = model_ids(&dialog.current_models());
    let chat_ids = model_ids(
        &app.state
            .chat_state
            .available_providers
            .iter()
            .find(|entry| entry.name == "local")
            .expect("provider 仍在目录中")
            .models,
    );

    assert_eq!(
        dialog_ids,
        vec!["official-a", "official-b"],
        "官方列表替换内置列表"
    );
    assert_eq!(
        chat_ids, dialog_ids,
        "弹窗副本与 ChatState 必须落到同一份结果上"
    );
    assert!(matches!(
        model_list_state(&app, "local"),
        crate::chat::ModelListState::Ready { .. }
    ));
}

/// 失败落地：列表与选中项都不动，只记一条错误状态（"成功一次后永久替换"
/// 的另一面——失败永远不回退、也不清空）。
#[tokio::test]
async fn failed_fetch_keeps_models_and_records_error() {
    let mut app = test_app_with_llm("openai-compatible", "pin-me", "http://127.0.0.1:1/v1");
    app.open_provider_selection_dialog();
    let dialog = app.state.provider_dialog.as_ref().expect("弹窗打开");
    let before = model_ids(&dialog.current_models());
    let selected_before = dialog.selected_model;

    let session = app.state.model_fetch_session;
    app.apply_model_fetch_outcome(session, failed_outcome("openai-compatible", "连接失败"));

    let dialog = app.state.provider_dialog.as_ref().expect("弹窗仍然打开");
    assert_eq!(model_ids(&dialog.current_models()), before, "失败不得改动列表");
    assert_eq!(dialog.selected_model, selected_before);
    match model_list_state(&app, "openai-compatible") {
        crate::chat::ModelListState::Failed {
            message,
            hint_only: false,
            ..
        } => assert_eq!(message, "连接失败"),
        other => panic!("失败应记录为 Failed{{hint_only:false}}，实际 {other:?}"),
    }
}

/// 拉取在飞期间目录被重置（`/new`、快照恢复都会经 `build_chat_state` 重置）
/// 时写不进目录：此时必须回落 `Builtin`，否则下次打开弹窗会拿着内置列表
/// 声称"官方模型列表"，并在 TTL 内拒绝重新拉取。
#[tokio::test]
async fn outcome_for_provider_missing_from_catalog_falls_back_to_builtin() {
    let mut app = test_app_with_llm("local", "pin-me", "http://127.0.0.1:1/v1");
    app.open_provider_selection_dialog();

    app.state.chat_state.available_providers.clear();
    let session = app.state.model_fetch_session;
    app.apply_model_fetch_outcome(session, ready_outcome("local", &["official-a"]));

    assert!(
        matches!(
            model_list_state(&app, "local"),
            crate::chat::ModelListState::Builtin
        ),
        "目录里没有该 provider 时不得记成 Ready"
    );
}

/// 切 provider 才拉取：Providers 栏内上下键只在真正换了 provider 时触发；
/// Models 栏内上下键只换模型，不触发任何请求。
#[tokio::test]
async fn provider_switch_refetches_only_when_provider_changes() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body(r#"{"data":[{"id":"m1"}]}"#)
        .create_async()
        .await;
    // openrouter 的下一项是 minimax：profile 声明无 catalog，拉取会在发请求前
    // 就返回 Unsupported，因此这条用例不会碰到任何真实厂商端点。
    let mut app = test_app_with_llm("openrouter", "or-pin", &format!("{}/v1", server.url()));
    app.open_provider_selection_dialog();
    let session = app.state.model_fetch_session;
    app.apply_model_fetch_outcome(session, ready_outcome("openrouter", &["or-a"]));

    // Models 栏内移动：provider 未变 → 不重拉（Ready 保持原样）。
    app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
        .await
        .expect("switch focus to Models");
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await
        .expect("move inside Models");
    assert!(
        matches!(
            model_list_state(&app, "openrouter"),
            crate::chat::ModelListState::Ready { .. }
        ),
        "只在 Models 栏内移动不得触发重新拉取"
    );

    // Providers 栏内移到 minimax：provider 变了 → 从未拉过 → Loading。
    app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .await
        .expect("switch focus back to Providers");
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await
        .expect("move to next provider");

    assert_eq!(
        app.state
            .provider_dialog
            .as_ref()
            .and_then(|dialog| dialog.selected_provider_name()),
        Some("minimax")
    );
    assert!(
        model_list_state(&app, "minimax").is_loading(),
        "换到新 provider 应发起一次拉取"
    );
}

#[tokio::test]
async fn stale_loading_does_not_block_a_new_fetch() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/models")
        .expect(2)
        .with_status(200)
        .with_body(r#"{"data":[{"id":"m1"}]}"#)
        .create_async()
        .await;
    // provider 名用 `local`：只有与 `llm.provider` 一致时才会用配置里的 api_base，
    // 否则请求会打到真实厂商。
    let mut app = test_app_with_llm("local", "pin-me", &format!("{}/v1", server.url()));
    app.open_provider_selection_dialog();

    // 先让"打开弹窗"那次拉取正常落地，腾出在飞名额（否则下面的重试会被名额挡住，
    // 测试会变成竞态）。
    let (session, outcome) = app.model_fetch_rx.recv().await.expect("首次拉取应回传");
    app.apply_model_fetch_outcome(session, outcome);
    assert!(app.state.model_fetch_inflight.is_empty());

    // 模拟"任务 panic 导致结果永不回传"：状态停在 Loading，且已过兜底窗口。
    app.state.chat_state.set_model_list_state(
        "local",
        crate::chat::ModelListState::Loading {
            since: Instant::now() - (crate::chat::MODEL_LIST_STALE_AFTER + Duration::from_secs(1)),
        },
    );

    app.maybe_refresh_selected_provider();

    // 状态是同步置位的：立刻就能看到"新任务"的时间戳（不再是陈旧的）。
    let state = model_list_state(&app, "local");
    assert!(state.is_loading(), "重新拉起后仍是 Loading");
    assert!(
        !state.is_stale_loading(Instant::now()),
        "陈旧 Loading 必须能被重新拉起（时间戳应为新任务）"
    );

    // 等这次重试真的打出去，再核对请求数（否则断言跑在 HTTP 之前，是竞态）。
    let (session, outcome) = app.model_fetch_rx.recv().await.expect("重试必须回传结果");
    app.apply_model_fetch_outcome(session, outcome);
    mock.assert_async().await;
}

#[test]
fn outcomes_from_a_previous_session_are_dropped() {
    let mut app = test_app_with_llm("openai-compatible", "pin-me", "http://127.0.0.1:1/v1");
    let stale_session = app.state.model_fetch_session;
    app.state.next_model_fetch_session(); // 模拟 `/new` 重建了会话

    let outcome = || crate::provider_service::ProviderModelFetchOutcome {
        provider: "openai-compatible".to_string(),
        result: Ok(vec![crate::chat::ModelInfo {
            id: "m1".to_string(),
            name: "m1".to_string(),
        }]),
    };

    app.apply_model_fetch_outcome(stale_session, outcome());
    assert!(
        !matches!(
            model_list_state(&app, "openai-compatible"),
            crate::chat::ModelListState::Ready { .. }
        ),
        "旧会话的结果不得写进新会话（否则会带着官方列表并在 TTL 内拒绝刷新）"
    );
    assert!(
        app.state.model_fetch_inflight.is_empty(),
        "被丢弃的结果同样要释放在飞名额"
    );

    // 对照组：当代的结果必须照常落地，证明上面的断言不是空转。
    let current = app.state.model_fetch_session;
    app.apply_model_fetch_outcome(current, outcome());
    assert!(
        matches!(
            model_list_state(&app, "openai-compatible"),
            crate::chat::ModelListState::Ready { .. }
        ),
        "当代结果必须正常应用"
    );
}
