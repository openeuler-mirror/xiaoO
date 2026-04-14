use agent_contracts::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_types::common::BuildError;
use async_trait::async_trait;
use serde_json::Value;
use std::borrow::Cow;

pub(crate) const BACKEND_TYPE_NOOP: &str = "noop";
pub(crate) const BACKEND_TYPE_STDOUT: &str = "stdout";
pub(crate) const BACKEND_TYPE_MOIRAI_SQLITE: &str = "moirai-sqlite";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceBackendType {
    Noop,
    Stdout,
    MoiraiSqlite,
}

impl TraceBackendType {
    pub(crate) fn parse(value: &str) -> Result<Self, BuildError> {
        match value {
            BACKEND_TYPE_NOOP => Ok(Self::Noop),
            BACKEND_TYPE_STDOUT => Ok(Self::Stdout),
            BACKEND_TYPE_MOIRAI_SQLITE => Ok(Self::MoiraiSqlite),
            other => Err(BuildError::InvalidConfig {
                message: format!("unsupported trace backend: {other}"),
            }),
        }
    }
}

#[async_trait]
pub(crate) trait TraceBackend: Send + Sync {
    async fn begin_span(
        &self,
        occurred_at_ms: u64,
        span: &TraceSpanHandle,
        kind: TraceSpanKind,
        name: Cow<'static, str>,
        fields: Value,
    );

    async fn update_span(&self, occurred_at_ms: u64, span: &TraceSpanHandle, fields: Value);

    async fn end_span(
        &self,
        occurred_at_ms: u64,
        span: TraceSpanHandle,
        outcome: TraceOutcome,
        fields: Value,
    );

    async fn finalize_trace(&self, occurred_at_ms: u64, outcome: TraceOutcome, fields: Value);

    #[allow(dead_code)]
    async fn force_finalize_trace(&self, occurred_at_ms: u64, outcome: TraceOutcome, fields: Value);
}
