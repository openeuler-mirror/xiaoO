//! Configuration types for the cron subsystem.

use std::path::PathBuf;

/// Global cron configuration from `config.toml`'s `[cron]` section.
#[derive(Debug, Clone)]
pub struct CronGlobalConfig {
    /// Directory containing `jobs.toml`.
    pub jobs_dir: PathBuf,
    /// Maximum number of concurrently executing jobs (0 = unlimited).
    pub max_concurrent_jobs: usize,
    /// Default timeout in seconds for jobs that don't specify one.
    pub default_timeout_secs: u64,
}

/// Raw definition of a single cron job from `jobs.toml` (before merging globals).
#[derive(Debug, Clone)]
pub struct CronJobDef {
    /// Unique job identifier.
    pub name: String,
    /// Human-readable description for logging / UI.
    pub description: Option<String>,
    /// Validated cron expression.
    pub cron: super::CronExpression,
    /// Prompt text sent to the agent at trigger time.
    pub prompt: String,
    /// Optional agent role override (maps to `[agent.<name>]` in config).
    pub agent_role: Option<String>,
    /// Per-job timeout override.  `None` means "use the global default".
    pub timeout_secs: Option<u64>,
    /// Whether the job is enabled.
    pub enabled: bool,
    /// Maximum retry attempts on failure.
    pub max_retries: u32,
    /// Delay between retries (seconds).
    pub retry_delay_secs: u64,
}

/// Fully resolved runtime configuration for a single cron job.
///
/// All optional fields have been merged with global defaults.
#[derive(Debug, Clone)]
pub struct CronJobConfig {
    /// Unique job identifier.
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Validated cron expression.
    pub cron: super::CronExpression,
    /// Prompt text.
    pub prompt: String,
    /// Optional agent role.
    pub agent_role: Option<String>,
    /// Resolved timeout in seconds.
    pub timeout_secs: u64,
    /// Whether the job is enabled.
    pub enabled: bool,
    /// Maximum retry attempts.
    pub max_retries: u32,
    /// Delay between retries (seconds).
    pub retry_delay_secs: u64,
}
