use async_trait::async_trait;

/// Abstraction for sending files through a channel (e.g. Feishu).
///
/// This trait lives in agent-contracts so that tools can send files
/// without depending on any specific channel implementation.
#[async_trait]
pub trait ChannelFileSender: Send + Sync {
    /// Send a file to the current conversation.
    ///
    /// Returns the message ID on success, or an error description.
    async fn send_file(
        &self,
        file_path: &str,
        label: Option<&str>,
    ) -> Result<Option<String>, String>;

    /// The conversation ID this sender is bound to.
    fn conversation_id(&self) -> &str;
}
