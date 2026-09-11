use super::*;
use agent_llm::{ChatMessageExt, LlmRequestExt};
use agent_types::LlmRequest;

fn make_provider() -> AnthropicProvider {
    make_provider_for_model("claude-sonnet-4-6")
}

fn make_provider_for_model(model: &str) -> AnthropicProvider {
    AnthropicProvider::new(
        "test-key".to_string(),
        "https://api.anthropic.com/v1".to_string(),
        model.to_string(),
    )
}

#[test]
fn test_parse_content_block_delta() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        "event: content_block_delta",
    )
    .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
    )
    .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().content, Some("Hello".to_string()));
}

#[test]
fn test_parse_message_delta() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: message_delta")
        .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#,
    )
    .unwrap();
    let chunk = result.unwrap();
    assert_eq!(chunk.finish_reason, Some("end_turn".to_string()));
    assert!(chunk.usage.is_some());
    assert_eq!(chunk.usage.unwrap().completion_tokens, 15);
}

#[test]
fn test_parse_message_start_usage() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: message_start")
        .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":21}}}"#,
    )
    .unwrap();
    let chunk = result.unwrap();
    assert!(chunk.usage.is_some());
    assert_eq!(chunk.usage.unwrap().prompt_tokens, 21);
}

#[test]
fn merge_usage_keeps_prompt_and_completion_totals() {
    let merged = merge_usage(
        Some(Usage {
            prompt_tokens: 21,
            completion_tokens: 0,
            total_tokens: 21,
            cached_tokens: 0,
        }),
        Usage {
            prompt_tokens: 0,
            completion_tokens: 15,
            total_tokens: 15,
            cached_tokens: 0,
        },
    );

    assert_eq!(merged.prompt_tokens, 21);
    assert_eq!(merged.completion_tokens, 15);
    assert_eq!(merged.total_tokens, 36);
}

#[test]
fn test_parse_message_stop() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: message_stop")
        .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"message_stop"}"#,
    )
    .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_parse_input_json_delta_as_tool_call() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        "event: content_block_delta",
    )
    .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":\"Tok"}}"#,
    )
    .unwrap();
    let chunk = result.unwrap();
    let tool_calls = chunk.tool_calls.unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].index, 1);
    assert_eq!(
        tool_calls[0].function.as_ref().unwrap().arguments,
        Some("{\"location\":\"Tok".to_string())
    );
}

#[test]
fn test_parse_content_block_stop() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: content_block_stop")
        .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"content_block_stop","index":0}"#,
    )
    .unwrap();
    assert!(result.is_some());
    let chunk = result.unwrap();
    assert!(chunk.content.is_none());
    assert!(chunk.finish_reason.is_none());
    assert!(chunk.usage.is_none());
}

#[test]
fn build_body_emits_system_blocks_caching_only_the_stable_prefix() {
    let provider = make_provider();
    let request = LlmRequest {
        messages: vec![
            agent_types::ChatMessage::system("base system"),
            agent_types::ChatMessage::system("# Context\n\nvolatile tail"),
            agent_types::ChatMessage::user("hello"),
        ],
        tools: Vec::new(),
        tool_choice: agent_types::ToolChoice::Auto,
        max_tokens: None,
        temperature: None,
        response_format: agent_types::ResponseFormat::Text,
        reasoning_effort: agent_types::ReasoningEffort::Off,
    };

    let body = provider.build_body(&request, false);

    let system = body["system"]
        .as_array()
        .expect("system should be an array");
    assert_eq!(system.len(), 2);
    // Stable prefix carries the cache breakpoint; the volatile tail does not,
    // so a per-turn change to the tail never invalidates the cached prefix.
    assert_eq!(system[0]["text"], "base system");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(system[1]["text"], "# Context\n\nvolatile tail");
    assert!(system[1].get("cache_control").is_none());
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn build_body_marks_last_two_messages_as_cache_breakpoints() {
    let provider = make_provider();
    let request = LlmRequest {
        messages: vec![
            agent_types::ChatMessage::system("base system"),
            agent_types::ChatMessage::user("first question"),
            agent_types::ChatMessage::assistant("first answer", 0),
            agent_types::ChatMessage::user("second question"),
            agent_types::ChatMessage::user(
                "<system-reminder>\n# Context\nper-turn state\n</system-reminder>",
            ),
        ],
        tools: Vec::new(),
        tool_choice: agent_types::ToolChoice::Auto,
        max_tokens: None,
        temperature: None,
        response_format: agent_types::ResponseFormat::Text,
        reasoning_effort: agent_types::ReasoningEffort::Off,
    };

    let body = provider.build_body(&request, false);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);

    // Older history carries no breakpoint.
    assert!(messages[0]["content"][0].get("cache_control").is_none());
    assert!(messages[1]["content"][0].get("cache_control").is_none());
    // The last two messages do: the second-to-last lands on stable
    // history (cache hit next turn); the last covers the ephemeral
    // per-turn reminder tail.
    assert_eq!(
        messages[2]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
    assert_eq!(
        messages[3]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

#[test]
fn build_body_caches_single_system_block() {
    let provider = make_provider();
    let request = LlmRequest {
        messages: vec![
            agent_types::ChatMessage::system("base system only"),
            agent_types::ChatMessage::user("hello"),
        ],
        tools: Vec::new(),
        tool_choice: agent_types::ToolChoice::Auto,
        max_tokens: None,
        temperature: None,
        response_format: agent_types::ResponseFormat::Text,
        reasoning_effort: agent_types::ReasoningEffort::Off,
    };

    let body = provider.build_body(&request, false);

    let system = body["system"]
        .as_array()
        .expect("system should be an array");
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["text"], "base system only");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn build_body_sets_thinking_budget_for_reasoning_effort() {
    let provider = make_provider();
    let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
    request.max_tokens = Some(4096);
    request.reasoning_effort = agent_types::ReasoningEffort::Max;

    let body = provider.build_body(&request, false);

    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 2048);
}

#[test]
fn build_body_omits_thinking_when_reasoning_effort_is_off() {
    let provider = make_provider();
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

    let body = provider.build_body(&request, false);

    assert!(body.get("thinking").is_none());
}

#[test]
fn claude_5_capability_uses_known_context_window() {
    let provider = make_provider_for_model("claude-sonnet-5");

    assert_eq!(provider.capabilities.max_context_window, 1_000_000);
}

#[test]
fn sonnet_5_disables_thinking_when_reasoning_effort_is_off() {
    let provider = make_provider_for_model("claude-sonnet-5");
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

    let body = provider.build_body(&request, false);

    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body.get("output_config").is_none());
}

#[test]
fn fable_5_does_not_request_unsupported_disabled_thinking() {
    let provider = make_provider_for_model("claude-fable-5");
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

    let body = provider.build_body(&request, false);

    assert!(body.get("thinking").is_none());
    assert!(body.get("output_config").is_none());
}

#[test]
fn claude_5_uses_adaptive_thinking_and_model_effort() {
    let provider = make_provider_for_model("anthropic/claude-sonnet-5");
    let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
    request.reasoning_effort = ReasoningEffort::Max;

    let body = provider.build_body(&request, false);

    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "max");
    assert!(body["thinking"].get("budget_tokens").is_none());
}

#[test]
fn claude_5_merges_structured_output_format_with_effort() {
    let provider = make_provider_for_model("claude-fable-5");
    let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
    request.reasoning_effort = ReasoningEffort::High;
    request.response_format = agent_types::ResponseFormat::JsonSchema {
        name: "answer".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } }
        }),
    };

    let body = provider.build_body(&request, false);

    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
}

#[test]
fn refusal_maps_to_content_filter_and_uses_explanation_text() {
    assert!(matches!(
        anthropic_stop_reason("refusal"),
        StopReason::ContentFilter
    ));
    let response = serde_json::json!({
        "content": [],
        "stop_reason": "refusal",
        "stop_details": { "explanation": "I can't help with that request." }
    });

    assert_eq!(
        anthropic_response_text(&response).as_deref(),
        Some("I can't help with that request.")
    );
}

#[test]
fn test_parse_unknown_event() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "event: unknown_event")
        .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"unknown_event"}"#,
    )
    .unwrap();
    assert!(result.is_some());
    let chunk = result.unwrap();
    assert!(chunk.content.is_none());
}

#[test]
fn test_parse_malformed_data() {
    let mut current_event = None;
    AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        "event: content_block_delta",
    )
    .unwrap();
    let result =
        AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "data: invalid json")
            .unwrap();
    assert!(result.is_some());
    let chunk = result.unwrap();
    assert!(chunk.content.is_none());
}

#[test]
fn test_parse_empty_line() {
    let mut current_event = None;
    let result = AnthropicProvider::parse_anthropic_stream_line(&mut current_event, "").unwrap();
    assert!(result.is_some());
    let chunk = result.unwrap();
    assert!(chunk.content.is_none());
}

#[test]
fn test_multiple_content_deltas() {
    let mut current_event = None;

    AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        "event: content_block_delta",
    )
    .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
    )
    .unwrap();
    assert_eq!(result.unwrap().content, Some("Hello".to_string()));

    AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        "event: content_block_delta",
    )
    .unwrap();
    let result = AnthropicProvider::parse_anthropic_stream_line(
        &mut current_event,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" World"}}"#,
    )
    .unwrap();
    assert_eq!(result.unwrap().content, Some(" World".to_string()));
}
