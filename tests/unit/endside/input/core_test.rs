use super::{EventHandler, Input};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

#[test]
fn typing_after_click_keeps_all_chars() {
    // Regression: a mouse click leaves anchor == cursor (an empty,
    // invisible selection). insert_char used to keep that anchor, so the
    // second typed char replaced the first via the selection-replace
    // path — typing "ab" produced "b".
    let mut input = Input::default().with_value(String::new());
    input.set_cursor(0);
    input.set_anchor(); // click artifact
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    )));
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('b'),
        KeyModifiers::NONE,
    )));

    assert_eq!(input.value(), "ab");
    assert!(input.selected_text().is_none());
    assert_eq!(input.cursor(), 2);
}

#[test]
fn backspace_key_deletes_previous_character() {
    let mut input = Input::default().with_value("hello".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Backspace,
        KeyModifiers::NONE,
    )));

    assert_eq!(input.value(), "hell");
    assert_eq!(input.cursor(), 4);
}

#[test]
fn ctrl_h_is_treated_as_backspace() {
    let mut input = Input::default().with_value("hello".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(input.value(), "hell");
    assert_eq!(input.cursor(), 4);
}

#[test]
fn del_control_character_is_treated_as_backspace() {
    let mut input = Input::default().with_value("hello".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('\u{7f}'),
        KeyModifiers::NONE,
    )));

    assert_eq!(input.value(), "hell");
    assert_eq!(input.cursor(), 4);
}

#[test]
fn esc_as_char_is_not_inserted() {
    // Some terminals/forwarders surface raw ESC (0x1b) as a `Char`
    // event instead of `KeyCode::Esc`. It must never reach the line.
    let mut input = Input::default().with_value("hello".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('\u{1b}'),
        KeyModifiers::NONE,
    )));

    assert_eq!(input.value(), "hello");
    assert_eq!(input.cursor(), 5);
}

#[test]
fn nul_as_char_is_not_inserted() {
    let mut input = Input::default().with_value("hello".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('\u{0}'),
        KeyModifiers::NONE,
    )));

    assert_eq!(input.value(), "hello");
}

#[test]
fn printable_chars_still_insert() {
    let mut input = Input::default();
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    )));
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('A'),
        KeyModifiers::SHIFT,
    )));
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('['),
        KeyModifiers::NONE,
    )));

    assert_eq!(input.value(), "aA[");
}

#[test]
fn ctrl_a_still_selects_all() {
    // Regression guard: the ESC-remnant fix must not drop Ctrl+A
    // (select-all feeds Ctrl+C copy in the input box).
    let mut input = Input::default().with_value("hello world".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(input.selected_text().as_deref(), Some("hello world"));
}

#[test]
fn select_all_handles_multibyte_chars() {
    // selected_text converts char indices to byte offsets; CJK chars
    // are 3 bytes each, so a full selection must return the whole
    // value, not a truncated one.
    let mut input = Input::default().with_value("你好 world".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(input.selected_text().as_deref(), Some("你好 world"));
}
