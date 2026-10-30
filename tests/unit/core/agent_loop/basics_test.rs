#[test]
fn assistant_with_tool_calls(calls: &[(&str, &str)]) -> AssistantMessage {
    AssistantMessage {
        text: None,
        reasoning_content: None,
        tool_calls: calls
            .iter()
            .map(|(call_id, tool_name)| ToolUseBlock {
                call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
                input: serde_json::json!({}),
            })
            .collect(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
    }
}

#[test]
fn synthesize_missing_call_ids_fills_only_empty_ids_uniquely() {
    let mut msg = assistant_with_tool_calls(&[
        ("", "grep"),          // missing id → synthesized
        ("   ", "bash"),       // whitespace-only id → treated as missing
        ("call_real", "edit"), // provider-supplied id → preserved
    ]);

    synthesize_missing_call_ids(&mut msg, 3);

    assert_eq!(msg.tool_calls[0].call_id, "call_3_0");
    assert_eq!(msg.tool_calls[1].call_id, "call_3_1");
    assert_eq!(msg.tool_calls[2].call_id, "call_real");
    assert!(msg.tool_calls.iter().all(is_valid_tool_call));
    assert_ne!(msg.tool_calls[0].call_id, msg.tool_calls[1].call_id);

    let mut next = assistant_with_tool_calls(&[("", "grep")]);
    synthesize_missing_call_ids(&mut next, 4);
    assert_eq!(next.tool_calls[0].call_id, "call_4_0");
    assert_ne!(next.tool_calls[0].call_id, msg.tool_calls[0].call_id);
}

#[test]
fn synthesize_missing_call_ids_does_not_rescue_empty_tool_name() {
    let mut msg = assistant_with_tool_calls(&[("", "")]);
    synthesize_missing_call_ids(&mut msg, 0);
    assert_eq!(msg.tool_calls[0].call_id, "call_0_0");
    assert!(!is_valid_tool_call(&msg.tool_calls[0]));
}

#[test]
fn compression_trigger_as_str() {
    assert_eq!(CompressionTrigger::Automatic.as_str(), "automatic");
    assert_eq!(
        CompressionTrigger::ContextLimitRetry.as_str(),
        "context_limit_retry"
    );
    assert_eq!(
        CompressionTrigger::PreCheckExceeded.as_str(),
        "pre_check_exceeded"
    );
}

#[test]
fn compression_trigger_is_forced() {
    assert!(!CompressionTrigger::Automatic.is_forced());
    assert!(CompressionTrigger::ContextLimitRetry.is_forced());
    assert!(CompressionTrigger::PreCheckExceeded.is_forced());
}

mod token_budget_tests {
    use super::*;
    use crate::token_estimator::TokenEstimator;
    use agent_types::MessageRole;

    #[test]
    fn estimator_counts_positive_tokens_for_messages() {
        let estimator = TokenEstimator::new();
        let messages = vec![
            ChatMessage::text(MessageRole::User, "Hello world", 0),
            ChatMessage::text(MessageRole::Assistant, "Hi there", 0),
        ];

        let estimated =
            estimator.estimate_input_tokens("You are a helpful assistant", 0, &messages);

        assert!(estimated > 0);
    }
}
