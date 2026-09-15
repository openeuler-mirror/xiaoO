#[test]
fn transient_backoff_grows_exponentially_without_retry_after() {
    assert_eq!(transient_backoff(0, 0), Duration::from_millis(4_000));
    assert_eq!(transient_backoff(1, 0), Duration::from_millis(8_000));
    assert_eq!(transient_backoff(2, 0), Duration::from_millis(16_000));
    assert_eq!(transient_backoff(3, 0), Duration::from_millis(32_000));
}

#[test]
fn transient_backoff_is_clamped_to_the_ceiling() {
    assert_eq!(transient_backoff(10, 0), Duration::from_millis(60_000));
    assert_eq!(
        transient_backoff(u32::MAX, 0),
        Duration::from_millis(60_000)
    );
}

#[test]
fn transient_backoff_honors_retry_after_within_the_ceiling() {
    assert_eq!(transient_backoff(3, 5_000), Duration::from_millis(5_000));
    assert_eq!(transient_backoff(0, 120_000), Duration::from_millis(60_000));
}

#[test]
fn is_transient_retries_network_and_throttle_errors_only() {
    assert!(is_transient(&LlmError::HttpError(
        "error sending request for url".into()
    )));
    assert!(is_transient(&LlmError::Timeout));
    assert!(is_transient(&LlmError::RateLimited {
        retry_after_ms: 0,
        message: String::new(),
    }));

    assert!(!is_transient(&LlmError::AuthError {
        message: String::new(),
    }));
    assert!(!is_transient(&LlmError::ApiError("HTTP 400".into())));
    assert!(!is_transient(&LlmError::ContextLengthExceeded {
        message: String::new(),
    }));
}

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
    fn test_estimator_basic_calculation() {
        let estimator = TokenEstimator::new();
        let messages = vec![
            ChatMessage::text(MessageRole::User, "Hello world", 0),
            ChatMessage::text(MessageRole::Assistant, "Hi there", 0),
        ];

        let estimated =
            estimator.estimate_input_tokens("You are a helpful assistant", 0, &messages);

        assert!(estimated > 0);
    }

    #[test]
    fn test_budget_calculation_logic() {
        let config = TokenBudgetConfig {
            total_budget: 10000,
            reserved_for_output: 1000,
            reserved_for_system: 500,
            hard_limit_ratio: 0.8,
        };

        let available_for_input = config
            .total_budget
            .saturating_sub(config.reserved_for_output)
            .saturating_sub(config.reserved_for_system);

        assert_eq!(available_for_input, 8500);
    }

    #[test]
    fn test_budget_edge_case_zero_available() {
        let config = TokenBudgetConfig {
            total_budget: 1000,
            reserved_for_output: 1000,
            reserved_for_system: 500,
            hard_limit_ratio: 0.8,
        };

        let available_for_input = config
            .total_budget
            .saturating_sub(config.reserved_for_output)
            .saturating_sub(config.reserved_for_system);

        assert_eq!(available_for_input, 0);
    }

    #[test]
    fn test_budget_edge_case_over_allocation() {
        let config = TokenBudgetConfig {
            total_budget: 1000,
            reserved_for_output: 800,
            reserved_for_system: 400,
            hard_limit_ratio: 0.8,
        };

        let available_for_input = config
            .total_budget
            .saturating_sub(config.reserved_for_output)
            .saturating_sub(config.reserved_for_system);

        assert_eq!(available_for_input, 0);

        let is_invalid =
            config.reserved_for_output + config.reserved_for_system >= config.total_budget;
        assert!(is_invalid);
    }
}
