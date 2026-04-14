use agent_contracts::ToolEventSink;
use agent_types::events::ToolLifecycleEvent;

pub struct PrintStdoutToolEventSink;

impl ToolEventSink for PrintStdoutToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        println!("[tool-cli][tool_event] {:?}", event);
    }
}
