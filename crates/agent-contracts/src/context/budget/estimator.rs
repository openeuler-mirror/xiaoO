use agent_types::ChatMessage;

pub trait TokenEstimator: Send + Sync {
    fn estimate_message_tokens(&self, message: &ChatMessage) -> usize;
    fn estimate_messages_tokens(&self, messages: &[ChatMessage]) -> usize;
    fn estimate_text_tokens(&self, text: &str) -> usize;
}
