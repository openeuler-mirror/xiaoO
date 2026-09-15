use super::*;
use crate::chat::Message;

#[test]
fn empty_messages_returns_none() {
    assert!(DeleteDialog::new(&[]).is_none());
}

#[test]
fn system_only_messages_returns_none() {
    let msgs = vec![Message::system("hello")];
    assert!(DeleteDialog::new(&msgs).is_none());
}

#[test]
fn single_user_message_creates_one_turn() {
    let msgs = vec![
        Message::system("welcome"),
        Message::user("help me"),
        Message::assistant_streaming(),
    ];
    let dialog = DeleteDialog::new(&msgs).unwrap();
    let entries = match &dialog {
        DeleteDialog::Selecting { entries, .. } => entries,
        _ => panic!("expected Selecting"),
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].full_content, "help me");
    // msg_range should be [1, 3) — skip the system message
    assert_eq!(entries[0].msg_range, (1, 3));
}

#[test]
fn multiple_turns_group_correctly() {
    let msgs = vec![
        Message::user("turn 1 question"),
        Message::assistant_streaming(),
        Message::user("turn 2 question"),
        Message::assistant_streaming(),
    ];
    let dialog = DeleteDialog::new(&msgs).unwrap();
    let entries = match &dialog {
        DeleteDialog::Selecting { entries, .. } => entries,
        _ => panic!("expected Selecting"),
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].msg_range, (0, 2));
    assert_eq!(entries[1].msg_range, (2, 4));
}

#[test]
fn advance_to_confirm_transitions_state() {
    let msgs = vec![
        Message::user("q1"),
        Message::assistant_streaming(),
        Message::user("q2"),
    ];
    let mut dialog = DeleteDialog::new(&msgs).unwrap();
    assert!(dialog.is_selecting());

    dialog.advance_to_confirm();
    assert!(!dialog.is_selecting());
    let (subsequent, content) = match &dialog {
        DeleteDialog::Confirming {
            turn,
            subsequent_count,
        } => (*subsequent_count, turn.full_content.clone()),
        _ => panic!("expected Confirming"),
    };
    assert_eq!(subsequent, 1); // q2 is after q1
    assert_eq!(content, "q1");
}

#[test]
fn long_content_is_preserved_in_full() {
    let long_msg = "a".repeat(100);
    let msgs = vec![Message::user(long_msg.clone())];
    let dialog = DeleteDialog::new(&msgs).unwrap();
    let entries = match &dialog {
        DeleteDialog::Selecting { entries, .. } => entries,
        _ => panic!("expected Selecting"),
    };
    assert_eq!(entries[0].full_content, long_msg);
}
