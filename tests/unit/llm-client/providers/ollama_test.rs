use super::*;

#[test]
fn usage_from_ollama_json_maps_eval_counts() {
    let json = serde_json::json!({
        "prompt_eval_count": 32,
        "eval_count": 18,
    });

    let usage = usage_from_ollama_json(&json);

    assert_eq!(usage.prompt_tokens, 32);
    assert_eq!(usage.completion_tokens, 18);
    assert_eq!(usage.total_tokens, 50);
}

#[test]
fn parse_ollama_stream_line_extracts_content_and_usage() {
    let parsed = parse_ollama_stream_line(
        r#"{"message":{"content":"hello"},"done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":7}"#,
    )
    .expect("ollama stream line should parse")
    .expect("parsed chunk expected");

    assert_eq!(parsed.content, Some("hello".to_string()));
    assert_eq!(parsed.finish_reason, Some("stop".to_string()));
    let usage = parsed.usage.expect("usage should be extracted");
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.completion_tokens, 7);
    assert_eq!(usage.total_tokens, 19);
}
