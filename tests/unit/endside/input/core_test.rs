use super::{handle_input_key, EventHandler, Input};
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

// ── readline (emacs) editing shortcuts ─────────────────────────────────────
// The bindings are hard-coded in `handle_input_key`; there is no keymap
// configuration.

#[test]
fn ctrl_a_moves_to_line_start() {
    // readline convention: ^A = beginning of line, NOT select-all.
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(8);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
    );

    assert_eq!(input.cursor(), 0);
    assert!(input.selected_text().is_none());
}

#[test]
fn multibyte_selection_reports_full_text() {
    // selected_text converts char indices to byte offsets; CJK chars
    // are 3 bytes each, so a full selection must return the whole
    // value, not a truncated one.
    let mut input = Input::default().with_value("你好 world".to_string());
    input.set_cursor(0);
    input.set_anchor();
    input.set_cursor(8);

    assert_eq!(input.selected_text(), Some("你好 world"));
}

#[test]
fn ctrl_e_moves_to_line_end() {
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(2);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
    );

    assert_eq!(input.cursor(), 11);
}

#[test]
fn ctrl_u_and_ctrl_k_kill_to_line_edges() {
    // Ctrl+U: kill back to line start, caret stays home.
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(8);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "rld");

    // Ctrl+K: kill forward to line end.
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(6);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "hello ");
}

#[test]
fn ctrl_k_kills_through_the_line_newline() {
    // readline/Emacs Ctrl+K semantics: the trailing newline is killed too,
    // so the following line joins the current one.

    // Mid-line: kill the rest of the line AND its newline → next line joins.
    let mut input = Input::default().with_value("ab\ncd".to_string());
    input.set_cursor(1);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "acd");
    assert_eq!(input.cursor(), 1);

    // At end of line (caret right before the newline): Ctrl+K joins the
    // lines by deleting the newline.
    let mut input = Input::default().with_value("ab\ncd".to_string());
    input.set_cursor(2);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "abcd");
    assert_eq!(input.cursor(), 2);

    // Final line has no trailing newline: unchanged behavior (to buffer end).
    let mut input = Input::default().with_value("ab\ncd".to_string());
    input.set_cursor(3);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "ab\n");
    assert_eq!(input.cursor(), 3);
}

#[test]
fn ctrl_w_deletes_word_backward() {
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(11);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "hello ");
    assert_eq!(input.cursor(), 6);
}

#[test]
fn alt_b_f_move_by_word() {
    // Alt+F: to start of next word.
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(0);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
    );
    assert_eq!(input.cursor(), 6);

    // Alt+B: back to previous word start.
    let mut input = Input::default().with_value("hello world".to_string());
    input.set_cursor(11);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
    );
    assert_eq!(input.cursor(), 6);
}

#[test]
fn ctrl_d_deletes_forward_and_is_noop_at_line_end() {
    let mut input = Input::default().with_value("hello".to_string());
    input.set_cursor(2);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    );
    assert_eq!(input.value(), "helo");

    // Empty line: Ctrl+D must be a no-op (no EOF/quit semantics in-app).
    let mut empty = Input::default();
    handle_input_key(
        &mut empty,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    );
    assert_eq!(empty.value(), "");
}

#[test]
fn ctrl_shift_letter_is_never_a_bare_letter_edit() {
    // Modifier bits are compared exactly, so Ctrl+Shift+A is not Ctrl+A and is
    // simply ignored (never typed, never a line-start).
    let mut input = Input::default().with_value("hello".to_string());
    input.set_cursor(3);
    handle_input_key(
        &mut input,
        KeyEvent::new(
            KeyCode::Char('A'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
    );

    assert_eq!(input.value(), "hello");
    assert_eq!(input.cursor(), 3);
}

#[test]
fn unbound_alt_letter_is_dropped() {
    // Alt+Q is not bound: it must not type "q".
    let mut input = Input::default().with_value("hi".to_string());
    input.set_cursor(2);
    assert!(handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT)
    ));
    assert_eq!(input.value(), "hi");

    // AltGr (CONTROL|ALT) still inserts, like a plain character:
    let mut input = Input::default().with_value("".to_string());
    input.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )));
    assert_eq!(input.value(), "@");
}

#[test]
fn app_level_actions_fall_through_to_plain_handling() {
    // Submit / newline / history / copy / cut are NOT consumed by the Input
    // widget; callers resolve them at the app layer.
    let mut input = Input::default().with_value("abc".to_string());
    input.set_cursor(1);
    assert!(!handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    ));
    assert_eq!(input.value(), "abc");
    assert_eq!(input.cursor(), 1);
}

#[test]
fn inserted_tab_keeps_one_character_and_four_display_cells() {
    let mut input = Input::default();
    input.handle(crate::input::InputRequest::InsertChar('\t'));
    assert_eq!(input.value().as_bytes(), b"\t");
    assert_eq!(input.cursor(), 1);
    assert_eq!(input.visual_cursor(), 4);
    assert_eq!(input.display_value(false), "    ");
}

#[test]
fn control_chars_dropped_on_insert_newline_kept() {
    let mut input = Input::default();
    input.handle(crate::input::InputRequest::InsertChar('a'));
    input.handle(crate::input::InputRequest::InsertChar('\u{8}')); // backspace byte
    input.handle(crate::input::InputRequest::InsertChar('\u{b}')); // vertical tab
    input.handle(crate::input::InputRequest::InsertChar('\u{7f}')); // DEL
    input.handle(crate::input::InputRequest::InsertChar('\n')); // explicit newline stays
    input.handle(crate::input::InputRequest::InsertChar('b'));
    assert_eq!(input.value(), "a\nb");
    assert_eq!(input.cursor(), 3);
}

#[test]
fn with_value_preserves_allowed_controls_and_filters_others() {
    let input = Input::default().with_value("a\tb\u{8}c\r\nd".to_string());
    assert_eq!(input.value(), "a\tbc\r\nd");
    assert_eq!(input.cursor(), 7);
}

#[test]
fn pasted_indented_block_keeps_tabs() {
    // End-to-end: pasting an indented rpath=( … ) block (the reported
    // repro) keeps every line but renders tab indentation as spaces.
    let mut input = Input::default();
    crate::render::paste_into_input(
        &mut input,
        "rpath=(\n\t\"/usr/lib64\",\n\t\"/lib64\",\n)\n)",
    );
    assert_eq!(
        input.value(),
        "rpath=(\n\t\"/usr/lib64\",\n\t\"/lib64\",\n)\n)"
    );
}

#[test]
fn paste_preserves_multiline_bytes_at_every_position() {
    for text in [
        "\tA\tB\t",
        "\rA\rB\r",
        "\nA\nB\n",
        " \t中\r\n文\t ",
        "\t\r\n",
    ] {
        let mut input = Input::default();
        crate::render::paste_into_input(&mut input, text);
        assert_eq!(input.value().as_bytes(), text.as_bytes());
        assert_eq!(Input::from(text).value().as_bytes(), text.as_bytes());
        input.set_cursor(0);
        input.set_anchor();
        input.set_cursor(text.chars().count());
        assert_eq!(input.selected_text(), Some(text));
        assert_eq!(input.delete_selected().as_deref(), Some(text));
        assert_eq!(input.value(), "");
    }
}

#[test]
fn deleting_preserved_controls_uses_original_character_indices() {
    let mut input = Input::from("A\t\r\nB");
    input.set_cursor(2);
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    );
    assert_eq!(input.value(), "A\r\nB");
    handle_input_key(
        &mut input,
        KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
    );
    assert_eq!(input.value(), "A\nB");
}

#[test]
fn tab_keys_do_not_insert_text() {
    let mut input = Input::from("A\tB");
    for code in [KeyCode::Tab, KeyCode::BackTab] {
        handle_input_key(&mut input, KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(input.value(), "A\tB");
        assert_eq!(input.cursor(), 3);
    }
}
