use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Span;

use super::config::ContextConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct PendingUpdate {
    pub(super) extras: serde_json::Value,
    pub(super) last_updated_at: Option<i64>,
    pub(super) end_time: Option<i64>,
}

pub(super) struct Inner {
    pub(super) trace_id: String,
    #[allow(dead_code)]
    pub(super) agent_id: String,
    pub(super) head_span_id: String,
    pub(super) root_span_start_time: i64,
    pub(super) open_spans: Vec<String>,
    pub(super) buffer: Vec<Span>,
    pub(super) pending_updates: HashMap<String, PendingUpdate>,
    pub(super) config: ContextConfig,
    pub(super) ended: bool,
}
