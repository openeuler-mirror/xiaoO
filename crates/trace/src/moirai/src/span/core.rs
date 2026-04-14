use serde::{Deserialize, Serialize};

use super::types::SpanType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Span {
    pub span_id: String,
    pub trace_id: String,
    pub parent_span_id: Option<String>,
    pub span_type: SpanType,
    pub start_time: i64,
    pub last_updated_at: i64,
    pub end_time: Option<i64>,
    pub extras: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpanHandle {
    pub span_id: String,
    pub trace_id: String,
}
