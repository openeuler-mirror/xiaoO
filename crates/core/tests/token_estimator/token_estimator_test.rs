#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::ChatMessageExt;
    use agent_types::MessageRole;

    #[test]
    fn estimate_text_quick_handles_chinese() {
        let estimator = TokenEstimator::new();
        let chinese_text = "这是一个中文测试文本";
        let tokens = estimator.estimate_text_quick(chinese_text);

        assert!(tokens > 0);
        assert!(tokens < chinese_text.len());
    }

    #[test]
    fn estimate_text_quick_handles_english() {
        let estimator = TokenEstimator::new();
        let english_text = "This is an English test text with multiple words";
        let tokens = estimator.estimate_text_quick(english_text);

        assert!(tokens > 0);
        assert!(tokens < english_text.len());
    }

    #[test]
    fn estimate_message_with_cached_tokens() {
        let estimator = TokenEstimator::new();
        let msg = ChatMessage::text(MessageRole::User, "Test message", 0);

        assert_eq!(msg.estimated_tokens, None);

        let estimated = estimator.estimate_message(&msg);
        assert!(estimated > 0);
    }

    #[test]
    fn estimate_input_tokens_sum_components() {
        let estimator = TokenEstimator::new();
        let messages = vec![
            ChatMessage::text(MessageRole::User, "Hello", 0),
            ChatMessage::text(MessageRole::Assistant, "Hi there", 0),
        ];

        let total = estimator.estimate_input_tokens("System prompt", 5, &messages);

        assert!(total > 0);
    }

    #[test]
    fn system_prompt_is_cached() {
        let estimator = TokenEstimator::new();

        let first = estimator.estimate_system_prompt("Test prompt");
        let second = estimator.estimate_system_prompt("Different prompt");

        assert_eq!(first, second);
    }

    #[test]
    fn tools_tokens_is_cached() {
        let estimator = TokenEstimator::new();

        let first = estimator.estimate_tools(5);
        let second = estimator.estimate_tools(10);

        assert_eq!(first, second);
    }
}
