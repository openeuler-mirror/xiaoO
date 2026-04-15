use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::interaction_prompt::UserPromptResult;
use crate::session_gateway::{SessionGateway, SessionTurnUpdate};

pub(super) const STREAM_REVEAL_CHARS_PER_TICK: usize = 1;

pub(super) struct PendingStreamDone {
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) messages: Vec<llm_client::ChatMessage>,
}

pub struct GatewayRuntime {
    pub(super) session_gateway: SessionGateway,
    pub(super) stream_rx: Option<UnboundedReceiver<SessionTurnUpdate>>,
    pub(crate) stream_message_index: Option<usize>,
    pub(super) stream_reveal_buffer: String,
    pub(super) pending_stream_done: Option<PendingStreamDone>,
    pub(super) cancel_flag: Option<Arc<AtomicBool>>,
    pub(super) request_start: Option<Instant>,
    pub(super) first_token_latency_recorded: bool,
    pub(super) interaction_reply_tx: Option<UnboundedSender<UserPromptResult>>,
}

impl GatewayRuntime {
    pub fn new() -> Self {
        Self {
            session_gateway: SessionGateway::new(),
            stream_rx: None,
            stream_message_index: None,
            stream_reveal_buffer: String::new(),
            pending_stream_done: None,
            cancel_flag: None,
            request_start: None,
            first_token_latency_recorded: false,
            interaction_reply_tx: None,
        }
    }
}
