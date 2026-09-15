use super::*;
use crate::services::command_loader::ExternalCommand;

fn external_commands() -> Vec<ExternalCommand> {
    vec![ExternalCommand {
        name: "review".to_string(),
        description: "Review code".to_string(),
        body: "Review this carefully.".to_string(),
    }]
}

#[test]
fn editing_newline_key_plain_enter_is_not_newline() {
    assert!(!editing_key_inserts_newline(
        KeyCode::Enter,
        event::KeyModifiers::empty()
    ));
}

#[test]
fn editing_newline_key_alt_enter_inserts_newline() {
    assert!(editing_key_inserts_newline(
        KeyCode::Enter,
        event::KeyModifiers::ALT
    ));
}

#[test]
fn editing_newline_key_ctrl_enter_inserts_newline() {
    // Some terminal emulators encode Ctrl+J as Enter+Control rather
    // than Char('j')+Control, so the Enter branch must accept Control.
    assert!(editing_key_inserts_newline(
        KeyCode::Enter,
        event::KeyModifiers::CONTROL
    ));
}

#[test]
fn editing_newline_key_ctrl_j_inserts_newline() {
    assert!(editing_key_inserts_newline(
        KeyCode::Char('j'),
        event::KeyModifiers::CONTROL
    ));
}

#[test]
fn editing_newline_key_ctrl_uppercase_j_inserts_newline() {
    assert!(editing_key_inserts_newline(
        KeyCode::Char('J'),
        event::KeyModifiers::CONTROL
    ));
}

#[test]
fn editing_newline_key_ctrl_shift_j_does_not_insert_newline() {
    // Shift must opt out so Ctrl+Shift+J is not silently hijacked.
    assert!(!editing_key_inserts_newline(
        KeyCode::Char('j'),
        event::KeyModifiers::CONTROL | event::KeyModifiers::SHIFT
    ));
}

#[test]
fn editing_newline_key_plain_j_is_not_newline() {
    assert!(!editing_key_inserts_newline(
        KeyCode::Char('j'),
        event::KeyModifiers::empty()
    ));
}

#[test]
fn editing_newline_key_alt_j_is_not_newline() {
    // Only Alt+Enter (not Alt+J) is a newline shortcut.
    assert!(!editing_key_inserts_newline(
        KeyCode::Char('j'),
        event::KeyModifiers::ALT
    ));
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
