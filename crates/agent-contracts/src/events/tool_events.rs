use agent_types::events::ToolLifecycleEvent;

pub trait ToolEventSink: Send + Sync {
    fn emit(&self, event: ToolLifecycleEvent);
}
