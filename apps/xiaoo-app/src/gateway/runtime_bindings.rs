use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink, ToolEventSink};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct SessionRuntimeBindings {
    pub loop_event_sink: Option<Arc<dyn LoopEventSink>>,
    pub tool_event_sink: Option<Arc<dyn ToolEventSink>>,
    pub interaction_handle: Option<Arc<dyn InteractionHandle>>,
    pub channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
}
