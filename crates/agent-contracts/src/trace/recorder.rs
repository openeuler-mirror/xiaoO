use std::borrow::Cow;

use agent_types::common::BuildError;
use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceSpanKind {
    Turn,
    PromptBuild,
    Compression,
    LlmCall,
    ToolCall,
    Hook,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceOutcome {
    Ok,
    Error,
    Denied,
    Cancelled,
}

#[async_trait]
pub trait TraceRecorderBuilder: Sized {
    fn default() -> Self;

    fn from_json(self, config: Value) -> Result<Self, BuildError>;

    async fn build(&self) -> Result<Box<dyn TraceRecorder>, BuildError>;
}

#[async_trait]
pub trait TraceRecorder: Send + Sync {
    async fn begin_span(
        &self,
        kind: TraceSpanKind,
        name: Cow<'static, str>,
        fields: Value,
    ) -> TraceSpanHandle;

    async fn update_span(&self, span: &TraceSpanHandle, fields: Value);

    async fn end_span(&self, span: TraceSpanHandle, outcome: TraceOutcome, fields: Value);

    async fn finalize_trace(&self, outcome: TraceOutcome, fields: Value);

    #[allow(dead_code)]
    async fn force_finalize_trace(&self, outcome: TraceOutcome, fields: Value);
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraceSpanHandle {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
}

impl TraceSpanHandle {
    pub fn new(
        trace_id: impl Into<String>,
        span_id: impl Into<String>,
        parent_span_id: Option<String>,
    ) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
            parent_span_id,
        }
    }

    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    pub fn span_id(&self) -> &str {
        &self.span_id
    }

    pub fn parent_span_id(&self) -> Option<&str> {
        self.parent_span_id.as_deref()
    }
}
