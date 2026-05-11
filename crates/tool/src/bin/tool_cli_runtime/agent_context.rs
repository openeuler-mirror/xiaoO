use std::path::PathBuf;

use agent_contracts::{AgentContext, ConversationView};
use agent_types::common::{AgentMetadata, WorkspaceRef};
use agent_types::ChatMessage;

pub struct MockConversationView {
    messages: Vec<ChatMessage>,
}

impl MockConversationView {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }
}

impl ConversationView for MockConversationView {
    fn recent_messages(&self, limit: usize) -> Vec<ChatMessage> {
        eprintln!(
            "[tool-cli][agent_context.conversation.recent_messages] limit={}",
            limit
        );
        let start = self.messages.len().saturating_sub(limit);
        self.messages[start..].to_vec()
    }

    fn message_count(&self) -> usize {
        eprintln!("[tool-cli][agent_context.conversation.message_count]");
        self.messages.len()
    }
}

pub struct MockAgentContext {
    conversation: MockConversationView,
    workspace: WorkspaceRef,
    metadata: AgentMetadata,
}

impl MockAgentContext {
    pub fn new() -> Self {
        Self {
            conversation: MockConversationView::new(),
            workspace: WorkspaceRef {
                root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            },
            metadata: AgentMetadata {
                agent_id: "tool_cli".to_string(),
                model: "tool-cli".to_string(),
                session_id: None,
            },
        }
    }
}

impl AgentContext for MockAgentContext {
    fn conversation(&self) -> &dyn ConversationView {
        eprintln!("[tool-cli][agent_context.conversation]");
        &self.conversation
    }

    fn workspace(&self) -> &WorkspaceRef {
        eprintln!(
            "[tool-cli][agent_context.workspace] root={}",
            self.workspace.root.display()
        );
        &self.workspace
    }

    fn metadata(&self) -> &AgentMetadata {
        eprintln!(
            "[tool-cli][agent_context.metadata] agent_id={} model={}",
            self.metadata.agent_id, self.metadata.model
        );
        &self.metadata
    }
}
