//! Cron job management service for the TUI.
//!
//! Reads and writes `jobs.toml` independently of the daemon process,
//! using the shared cron validation entry point (`xiaoo_shared::cron`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xiaoo_shared::cron::validate_cron_expr as validate_cron_expr_inner;

/// A cron job entry for TUI display and editing.
#[derive(Debug, Clone)]
pub struct CronJobEntry {
    /// Unique job name.
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Raw cron expression string (as written in jobs.toml).
    pub cron_raw: String,
    /// Whether the expression parsed successfully.
    pub cron_valid: bool,
    /// Prompt text sent to agent at trigger time.
    pub prompt: String,
    /// Optional agent role override.
    pub agent_role: Option<String>,
    /// Timeout in seconds.
    pub timeout_secs: u64,
    /// Whether the job is enabled.
    pub enabled: bool,
    /// Maximum retry attempts.
    pub max_retries: u32,
    /// Delay between retries (seconds).
    pub retry_delay_secs: u64,
}

/// Result of reading the full cron configuration.
#[derive(Debug, Clone)]
pub struct CronConfigSnapshot {
    /// Path to jobs.toml.
    pub jobs_file: PathBuf,
    /// Whether the cron section exists in config.toml.
    pub cron_section_present: bool,
    /// Default timeout from global config.
    pub default_timeout_secs: u64,
    /// Parsed jobs.
    pub jobs: Vec<CronJobEntry>,
}

// ── TOML representation ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobsToml {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    job: Vec<JobToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobToml {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    cron: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_delay_secs: Option<u64>,
}

/// Simple cron section from config.toml (only what the TUI needs).
#[derive(Debug, Clone, Default, Deserialize)]
struct CronSectionRaw {
    #[serde(default = "default_cron_jobs_dir")]
    jobs_dir: String,
    #[serde(default = "default_max_concurrent")]
    #[allow(dead_code)]
    max_concurrent_jobs: usize,
    #[serde(default = "default_cron_timeout")]
    default_timeout_secs: u64,
}

fn default_cron_jobs_dir() -> String {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/xiaoo/cron")
        .to_string_lossy()
        .into_owned()
}

fn default_max_concurrent() -> usize {
    3
}

fn default_cron_timeout() -> u64 {
    3600
}

// ── Public API ──────────────────────────────────────────────────

/// Load the full cron configuration snapshot from the current config.
///
/// `config_path` is the path to `config.toml`.
pub fn load_cron_snapshot(config_path: &Path) -> anyhow::Result<CronConfigSnapshot> {
    let config_content = fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("failed to read config.toml: {e}"))?;

    // Parse the [cron] section minimally
    let cron_section: Option<CronSectionRaw> = toml::from_str(&config_content)
        .ok()
        .and_then(|v: toml::Value| v.get("cron").cloned())
        .and_then(|v| CronSectionRaw::deserialize(v).ok());

    let (jobs_dir, default_timeout_secs, cron_section_present) = match cron_section {
        Some(cfg) => (cfg.jobs_dir, cfg.default_timeout_secs, true),
        None => (default_cron_jobs_dir(), default_cron_timeout(), false),
    };

    let jobs_dir = shellexpand::tilde(&jobs_dir).into_owned();
    let jobs_file = Path::new(&jobs_dir).join("jobs.toml");

    let jobs = if jobs_file.exists() {
        let content = fs::read_to_string(&jobs_file)
            .map_err(|e| anyhow::anyhow!("failed to read jobs.toml: {e}"))?;
        parse_jobs_toml(&content, default_timeout_secs)?
    } else {
        Vec::new()
    };

    Ok(CronConfigSnapshot {
        jobs_file,
        cron_section_present,
        default_timeout_secs,
        jobs,
    })
}

/// Write the given jobs back to `jobs.toml`.
pub fn write_jobs(snapshot: &CronConfigSnapshot, jobs: &[CronJobEntry]) -> anyhow::Result<()> {
    let toml_jobs: Vec<JobToml> = jobs
        .iter()
        .map(|j| JobToml {
            name: j.name.clone(),
            description: j.description.clone(),
            cron: j.cron_raw.clone(),
            prompt: j.prompt.clone(),
            agent_role: j.agent_role.clone(),
            timeout_secs: if j.timeout_secs == snapshot.default_timeout_secs {
                None
            } else {
                Some(j.timeout_secs)
            },
            enabled: if !j.enabled { Some(false) } else { None },
            max_retries: if j.max_retries != 0 {
                Some(j.max_retries)
            } else {
                None
            },
            retry_delay_secs: if j.retry_delay_secs != 60 {
                Some(j.retry_delay_secs)
            } else {
                None
            },
        })
        .collect();

    let jobs_toml = JobsToml { job: toml_jobs };

    let parent = snapshot
        .jobs_file
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid jobs file path"))?;
    fs::create_dir_all(parent).map_err(|e| anyhow::anyhow!("failed to create cron dir: {e}"))?;

    let content = toml::to_string_pretty(&jobs_toml)
        .map_err(|e| anyhow::anyhow!("failed to serialize jobs.toml: {e}"))?;

    fs::write(&snapshot.jobs_file, &content)
        .map_err(|e| anyhow::anyhow!("failed to write jobs.toml: {e}"))?;

    Ok(())
}

/// Validate a cron expression string. Returns `Ok(())` if valid.
pub fn validate_cron_expr(raw: &str) -> Result<(), String> {
    validate_cron_expr_inner(raw)
}

// ── Internal helpers ───────────────────────────────────────────

fn parse_jobs_toml(content: &str, default_timeout: u64) -> anyhow::Result<Vec<CronJobEntry>> {
    let jobs: JobsToml =
        toml::from_str(content).map_err(|e| anyhow::anyhow!("failed to parse jobs.toml: {e}"))?;

    jobs.job
        .into_iter()
        .map(|j| {
            let cron_valid = validate_cron_expr_inner(&j.cron).is_ok();
            Ok(CronJobEntry {
                name: j.name,
                description: j.description,
                cron_raw: j.cron,
                cron_valid,
                prompt: j.prompt,
                agent_role: j.agent_role,
                timeout_secs: j.timeout_secs.unwrap_or(default_timeout),
                enabled: j.enabled.unwrap_or(true),
                max_retries: j.max_retries.unwrap_or(0),
                retry_delay_secs: j.retry_delay_secs.unwrap_or(60),
            })
        })
        .collect()
}
