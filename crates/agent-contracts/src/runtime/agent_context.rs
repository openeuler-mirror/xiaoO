use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::ChatMessage;

pub trait AgentContext: Send + Sync {
    fn conversation(&self) -> &dyn ConversationView;
    fn workspace(&self) -> &WorkspaceRef;
    fn metadata(&self) -> &AgentMetadata;
}

pub trait ConversationView: Send + Sync {
    /// Returns recent messages from the conversation.
    /// Returns owned Vec to support Arc<RwLock> backed storage where
    /// returning a reference to locked data would be unsound.
    fn recent_messages(&self, limit: usize) -> Vec<ChatMessage>;
    fn message_count(&self) -> usize;
}
