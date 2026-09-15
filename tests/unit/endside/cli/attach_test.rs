use super::{parse_sse_event, take_sse_frame};

#[test]
fn take_sse_frame_extracts_frame_and_drains() {
    let mut buf = String::from("data: {\"type\":\"x\"}\n\nleftover");
    let frame = take_sse_frame(&mut buf).unwrap();
    assert_eq!(frame, "data: {\"type\":\"x\"}");
    assert_eq!(buf, "leftover");
}

#[test]
fn take_sse_frame_returns_none_without_blank_line() {
    let mut buf = String::from("data: partial");
    assert!(take_sse_frame(&mut buf).is_none());
    assert_eq!(buf, "data: partial");
}

#[test]
fn parse_sse_event_parses_data_json() {
    let frame = "data: {\"type\":\"text_delta\",\"delta\":\"hi\"}";
    let event = parse_sse_event(frame).unwrap();
    assert_eq!(event["type"], "text_delta");
    assert_eq!(event["delta"], "hi");
}

#[test]
fn parse_sse_event_joins_multi_line_data() {
    let frame = "data: {\"type\":\"done\",\ndata: \"reply\":\"ok\"}";
    let event = parse_sse_event(frame).unwrap();
    assert_eq!(event["type"], "done");
    assert_eq!(event["reply"], "ok");
}

#[test]
fn parse_sse_event_ignores_comments_and_blank_lines() {
    assert!(parse_sse_event(": keepalive\n\n").is_none());
}

#[test]
fn parse_sse_event_returns_none_for_malformed_json() {
    assert!(parse_sse_event("data: {not json").is_none());
}
