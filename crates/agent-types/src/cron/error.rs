//! Error types for cron parsing and execution.

/// Top-level cron error aggregating all sub-error kinds.
#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("cron parse error: {0}")]
    Parse(#[from] CronParseError),

    #[error("cron execution error: {0}")]
    Execution(#[from] CronExecutionError),

    #[error("cron I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cron config error: {0}")]
    Config(String),
}

// Re-export the expression-level parse error so callers only need
// `use agent_types::cron::CronParseError`.
pub use super::expression::CronParseError;

/// Errors that occur during job execution.
#[derive(Debug, thiserror::Error)]
pub enum CronExecutionError {
    #[error("job '{job_name}' timed out after {timeout_secs}s")]
    Timeout {
        job_name: String,
        timeout_secs: u64,
    },

    #[error("job '{job_name}' session error: {error}")]
    Session {
        job_name: String,
        error: String,
    },

    #[error("job '{job_name}' is disabled")]
    Disabled {
        job_name: String,
    },
}
