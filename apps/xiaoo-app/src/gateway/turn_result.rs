use agent_types::ChatMessage;

#[derive(Debug, Clone)]
pub struct AppTurnResult {
    pub raw_reply: String,
    pub visible_reply: String,
    pub messages: Vec<ChatMessage>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}
