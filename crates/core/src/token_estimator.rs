use std::sync::OnceLock;

use agent_types::{ChatMessage, ContentBlock};

pub struct TokenEstimator {
    system_prompt_tokens: OnceLock<usize>,
    tools_tokens: OnceLock<usize>,
}

impl TokenEstimator {
    pub fn new() -> Self {
        Self {
            system_prompt_tokens: OnceLock::new(),
            tools_tokens: OnceLock::new(),
        }
    }

    pub fn estimate_input_tokens(
        &self,
        system_prompt: &str,
        tools_count: usize,
        messages: &[ChatMessage],
    ) -> usize {
        let system_tokens = self.estimate_system_prompt(system_prompt);
        let tools_tokens = self.estimate_tools(tools_count);
        let history_tokens = self.estimate_history_messages(messages);

        system_tokens + tools_tokens + history_tokens
    }

    pub fn estimate_system_prompt(&self, prompt: &str) -> usize {
        *self.system_prompt_tokens.get_or_init(|| {
            self.estimate_text_precise(prompt)
        })
    }

    pub fn estimate_tools(&self, count: usize) -> usize {
        *self.tools_tokens.get_or_init(|| {
            count * 150 + 200
        })
    }

    pub fn estimate_history_messages(&self, messages: &[ChatMessage]) -> usize {
        messages.iter().map(|msg| self.estimate_message(msg)).sum()
    }

    pub fn estimate_message(&self, msg: &ChatMessage) -> usize {
        if let Some(cached) = msg.estimated_tokens {
            return cached;
        }

        let mut total = 0;
        for block in &msg.blocks {
            total += self.estimate_block(block);
        }

        if let Some(reasoning) = &msg.reasoning_content {
            total += self.estimate_text_quick(reasoning);
        }

        total + 10
    }

    fn estimate_block(&self, block: &ContentBlock) -> usize {
        match block {
            ContentBlock::Text { text } => self.estimate_text_quick(text),
            ContentBlock::ToolUse { input, .. } => {
                let json_str = serde_json::to_string(input).unwrap_or_default();
                self.estimate_text_quick(&json_str) + 20
            },
            ContentBlock::ToolResult { output, .. } => {
                self.estimate_text_quick(output) + 15
            },
            ContentBlock::Image { description } => {
                self.estimate_text_quick(description) + 100
            },
            ContentBlock::Document { description } => {
                self.estimate_text_quick(description) + 150
            },
        }
    }

    fn estimate_text_precise(&self, text: &str) -> usize {
        let char_count = text.chars().count();
        let byte_count = text.len();

        let ascii_ratio = if char_count > 0 {
            (byte_count as f64 / char_count as f64).min(4.0)
        } else {
            4.0
        };

        if ascii_ratio > 2.5 {
            (char_count / 2).max(1) + 20
        } else {
            (char_count / 4).max(1) + 20
        }
    }

    fn estimate_text_quick(&self, text: &str) -> usize {
        let char_count = text.chars().count();
        let has_chinese = text.chars().any(|c| c > '\u{7F}');

        if has_chinese {
            (char_count / 2) + 10
        } else {
            (char_count / 4) + 10
        }
    }

    pub fn update_message_cache(&self, msg: &mut ChatMessage) {
        if msg.estimated_tokens.is_none() {
            msg.estimated_tokens = Some(self.estimate_message(msg));
        }
    }
}

impl Default for TokenEstimator {
    fn default() -> Self {
        Self::new()
    }
}

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
        let msg = ChatMessage::text(
            MessageRole::User,
            "Test message",
            0,
        );

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

        let total = estimator.estimate_input_tokens(
            "System prompt",
            5,
            &messages,
        );

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
