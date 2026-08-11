pub mod compaction;
pub mod defaults;
pub mod envelope;
pub mod estimator;
pub mod manager;
pub mod microcompact;
pub mod passthrough;
pub mod policy;
pub mod summary;

pub use compaction::{
    CompactMode, CompactRequest, CompactionBoundary, CompactionDecision, CompactionResult,
    PartialDirection,
};
pub use defaults::{
    build_context_manager, CompactOverrides, DEFAULT_AUTO_COMPACT_RATIO, DEFAULT_BLOCKING_RATIO,
    DEFAULT_COLLAPSE_PRESERVE_TAIL, DEFAULT_PRESERVE_RECENT_MESSAGES, DEFAULT_SNIP_PRESERVE_TAIL,
    DEFAULT_SNIP_STALE_AFTER_MS, DEFAULT_STALE_TOOL_PAIR_AFTER_MS, DEFAULT_SUMMARY_LLM_MAX_TOKENS,
    DEFAULT_SUMMARY_MAX_TOKENS, DEFAULT_SUMMARY_PRESERVE_TAIL, DEFAULT_SUMMARY_TEMPERATURE,
    DEFAULT_WARNING_RATIO,
};
pub use envelope::{ContextBreakdown, ContextEnvelope, ContextSection};
pub use estimator::{RoughTokenEstimator, RoughTokenEstimatorConfig};
pub use manager::{ContextManager, ContextManagerConfig, SessionMemoryCompactionPolicy};
pub use microcompact::MicroCompactionPolicy;
pub use passthrough::PassthroughCompressionPipeline;
pub use policy::{CompactionPolicy, CompactionPolicyService, ContextThresholds};
pub use summary::{SummaryCompressionBudget, SummaryCompressionResult};

use agent_contracts::CompressionError;
use agent_types::{BudgetError, LlmError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("invalid configuration: {message}")]
    InvalidConfiguration { message: String },
    #[error("summary budget exhausted: {message}")]
    SummaryBudgetExhausted { message: String },
    #[error("summary parse failed: {message}")]
    SummaryParse { message: String },
    #[error("compaction boundary not found: {pivot_message_id}")]
    BoundaryNotFound { pivot_message_id: String },
    #[error(transparent)]
    InvalidBudget(#[from] BudgetError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),
}

pub type CompactResult<T> = Result<T, CompactError>;

impl From<CompactError> for CompressionError {
    fn from(error: CompactError) -> Self {
        match error {
            CompactError::Io(error) => CompressionError::Io(error),
            other => CompressionError::Failed(other.to_string()),
        }
    }
}
