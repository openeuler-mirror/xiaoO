use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::ChatMessage;

pub trait AgentContext: Send + Sync {
    fn conversation(&self) -> &dyn ConversationView;
    fn workspace(&self) -> &WorkspaceRef;
    fn metadata(&self) -> &AgentMetadata;
}

pub trait ConversationView: Send + Sync {
    fn recent_messages(&self, limit: usize) -> &[ChatMessage];
    fn message_count(&self) -> usize;
}
