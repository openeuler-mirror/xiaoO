use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Result, Span};

/// Summary information about a trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSummary {
    pub trace_id: String,
    pub span_count: usize,
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub root_span_type: String,
}

/// Trait for storing and retrieving spans
#[async_trait]
pub trait SpanStorage: Send + Sync {
    /// Insert a new span into storage
    async fn insert_span(&self, span: &Span) -> Result<()>;

    /// Update the end time of a span
    async fn update_span_end(
        &self,
        span_id: &str,
        end_time: i64,
        last_updated_at: i64,
    ) -> Result<()>;

    /// Update span extras with JSON merge and optionally update end_time
    ///
    /// Merges the provided `extras` JSON object into the existing span's extras.
    /// New fields override old fields with the same key, while other fields are preserved.
    /// If `end_time` is Some, also updates the span's end_time.
    ///
    /// Returns NotFound error if the span doesn't exist.
    async fn update_span_extras(
        &self,
        span_id: &str,
        extras: serde_json::Value,
        last_updated_at: i64,
        end_time: Option<i64>,
    ) -> Result<()>;

    /// Retrieve a span by ID
    async fn get_span(&self, span_id: &str) -> Result<Option<Span>>;

    /// Retrieve all spans for a given trace
    async fn get_trace_spans(&self, trace_id: &str) -> Result<Vec<Span>>;

    /// List all traces with optional limit
    async fn list_traces(&self, limit: usize) -> Result<Vec<TraceSummary>>;

    /// List only traces that are still alive (end_time is None)
    async fn list_alive_traces(&self, limit: usize) -> Result<Vec<TraceSummary>>;

    /// Get a trace ID that matches the given prefix
    /// Returns Ok(Some(trace_id)) if exactly one trace matches
    /// Returns Ok(None) if no traces match
    /// Returns Err with list of matches if multiple traces match the prefix
    async fn get_trace_by_prefix(&self, prefix: &str) -> Result<Option<String>>;

    async fn get_span_by_prefix(&self, prefix: &str) -> Result<Option<String>>;

    /// Get the last span ID of a trace (the most recently started span)
    /// Returns Ok(Some(span_id)) if the trace has spans
    /// Returns Ok(None) if the trace has no spans
    async fn get_last_span_id(&self, trace_id: &str) -> Result<Option<String>>;

    /// Count the number of spans in a trace
    async fn count_spans(&self, trace_id: &str) -> Result<usize>;
}
