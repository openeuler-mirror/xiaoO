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
    pub name: String,
    pub description: Option<String>,
    pub cron: super::CronExpression,
    pub prompt: String,
    /// Overrides `[agent.<name>]` when set.
    pub agent_role: Option<String>,
    /// `None` means "use the global default".
    pub timeout_secs: Option<u64>,
    pub enabled: bool,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
}

/// Fully resolved runtime configuration for a single cron job.
///
/// All optional fields have been merged with global defaults.
#[derive(Debug, Clone)]
pub struct CronJobConfig {
    pub name: String,
    pub description: Option<String>,
    pub cron: super::CronExpression,
    pub prompt: String,
    pub agent_role: Option<String>,
    /// Resolved from `CronJobDef::timeout_secs` or the global default.
    pub timeout_secs: u64,
    pub enabled: bool,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
}
