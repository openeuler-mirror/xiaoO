pub mod compaction;
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
