use super::*;
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
    assert!(copy_feedback(&app), "Ctrl+Insert must route to the copy path");
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
