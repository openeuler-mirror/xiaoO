use super::{format_conversation_id, parse_conversation_id};

#[test]
fn formats_topic_conversation_id() {
    assert_eq!(format_conversation_id(-100123, Some(42)), "-100123:42");
    assert_eq!(format_conversation_id(123, None), "123");
}

#[test]
fn parses_topic_conversation_id() {
    let parsed = parse_conversation_id("-100123:42").expect("conversation id should parse");
    assert_eq!(parsed.chat_id, -100123);
    assert_eq!(parsed.message_thread_id, Some(42));
}
