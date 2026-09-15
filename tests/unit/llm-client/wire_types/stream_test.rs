use super::*;

#[test]
fn test_parsed_chunk() {
    let chunk = ParsedChunk::default();
    assert!(chunk.content.is_none());
    assert!(chunk.finish_reason.is_none());
    assert!(chunk.usage.is_none());

    let usage = WireUsage {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        prompt_tokens_details: None,
    };
    let chunk = ParsedChunk {
        content: Some("Hello".to_string()),
        reasoning: None,
        finish_reason: Some("stop".to_string()),
        usage: Some(usage),
        tool_calls: None,
        kv_transfer_params: None,
    };
    assert_eq!(chunk.content, Some("Hello".to_string()));
    assert_eq!(chunk.finish_reason, Some("stop".to_string()));
    assert_eq!(chunk.usage.unwrap().total_tokens, 15);
}

#[test]
fn test_parsed_chunk_with_tool_calls() {
    let tool_call_delta = WireToolCallDelta {
        index: 0,
        id: Some("call_123".to_string()),
        call_type: Some("function".to_string()),
        function: Some(super::super::tool::WireToolCallFunctionDelta {
            name: Some("test".to_string()),
            arguments: Some("{\"a\":".to_string()),
        }),
    };

    let chunk = ParsedChunk {
        content: Some("Hello".to_string()),
        reasoning: None,
        finish_reason: None,
        usage: None,
        tool_calls: Some(vec![tool_call_delta]),
        kv_transfer_params: None,
    };

    assert!(chunk.tool_calls.is_some());
    let deltas = chunk.tool_calls.as_ref().unwrap();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].index, 0);
}
