use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink, ToolEventSink};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use xiaoo_api::runtime::PendingUserMessageSource;

#[derive(Clone, Default)]
pub struct SessionRuntimeBindings {
    pub loop_event_sink: Option<Arc<dyn LoopEventSink>>,
    pub tool_event_sink: Option<Arc<dyn ToolEventSink>>,
    pub interaction_handle: Option<Arc<dyn InteractionHandle>>,
    pub channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    pub pending_user_messages: Option<Arc<dyn PendingUserMessageSource>>,
    /// External cancellation token owned by the TUI. When set, the session
    /// actor uses this token instead of creating its own, so pressing Esc in
    /// the TUI can actually cancel the in-flight backend turn (and persist
    /// partial loop state via the `ReturnCancelled` path).
    pub cancel_token: Option<CancellationToken>,
}
