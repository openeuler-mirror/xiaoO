use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::daemon_config::DaemonConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CronIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CronJobReport {
    pub name: String,
    pub description: Option<String>,
    pub cron: String,
    pub prompt: String,
    pub agent_role: Option<String>,
    pub timeout_secs: u64,
    pub enabled: bool,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
    pub next_run_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CronReport {
    pub schema_version: u32,
    pub configured: bool,
    pub valid: bool,
    pub jobs_file: PathBuf,
    pub jobs_file_exists: bool,
    pub max_concurrent_jobs: usize,
    pub default_timeout_secs: u64,
    pub jobs: Vec<CronJobReport>,
    pub errors: Vec<CronIssue>,
}

pub fn cron_report(config: &DaemonConfig) -> CronReport {
    let configured = config.cron_section().is_some();
    let jobs_file = cron_jobs_file(config);
    let jobs_file_exists = jobs_file.exists();
    let (max_concurrent_jobs, default_timeout_secs) = config
        .cron_section()
        .map(|section| (section.max_concurrent_jobs, section.default_timeout_secs))
        .unwrap_or((3, 3600));

    if !configured {
        return CronReport {
            schema_version: 1,
            configured,
            valid: true,
            jobs_file,
            jobs_file_exists,
            max_concurrent_jobs,
            default_timeout_secs,
            jobs: Vec::new(),
            errors: Vec::new(),
        };
    }

    match config.resolve_cron_jobs() {
        Ok(jobs) => {
            let now = chrono::Utc::now();
            CronReport {
                schema_version: 1,
                configured,
                valid: true,
                jobs_file,
                jobs_file_exists,
                max_concurrent_jobs,
                default_timeout_secs,
                jobs: jobs
                    .into_iter()
                    .map(|job| CronJobReport {
                        name: job.name,
                        description: job.description,
                        cron: job.cron.to_string(),
                        prompt: job.prompt,
                        agent_role: job.agent_role,
                        timeout_secs: job.timeout_secs,
                        enabled: job.enabled,
                        max_retries: job.max_retries,
                        retry_delay_secs: job.retry_delay_secs,
                        next_run_ms: job
                            .enabled
                            .then(|| job.cron.next_after(now))
                            .flatten()
                            .map(|value| value.timestamp_millis()),
                    })
                    .collect(),
                errors: Vec::new(),
            }
        }
        Err(error) => CronReport {
            schema_version: 1,
            configured,
            valid: false,
            jobs_file,
            jobs_file_exists,
            max_concurrent_jobs,
            default_timeout_secs,
            jobs: Vec::new(),
            errors: vec![CronIssue {
                path: "cron.jobs".to_string(),
                code: "invalid_jobs_file".to_string(),
                message: error.to_string(),
            }],
        },
    }
}

fn cron_jobs_file(config: &DaemonConfig) -> PathBuf {
    let configured = config
        .cron_section()
        .map(|section| section.jobs_dir.as_str())
        .unwrap_or("~/.config/xiaoo/cron");
    expand_home(configured).join("jobs.toml")
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    if let Some(relative) = path.strip_prefix("~/") {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(relative);
    }
    Path::new(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::cron_report;
    use crate::daemon_config::DaemonConfig;

    #[test]
    fn reports_jobs_and_next_run_without_executing_them() {
        let temp = tempfile::tempdir().expect("temp dir");
        let jobs_dir = temp.path().join("cron");
        std::fs::create_dir_all(&jobs_dir).expect("jobs dir");
        std::fs::write(
            jobs_dir.join("jobs.toml"),
            r#"[[job]]
name = "daily-review"
description = "Review repository"
cron = "0 9 * * *"
prompt = "Review the current repository"
agent_role = "plan"
timeout_secs = 120
max_retries = 2
retry_delay_secs = 10
"#,
        )
        .expect("jobs file");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[cron]\njobs_dir = {:?}\nmax_concurrent_jobs = 2\ndefault_timeout_secs = 300\n",
                jobs_dir.to_string_lossy()
            ),
        )
        .expect("config");

        let report = cron_report(&DaemonConfig::load_from(&config_path).expect("load config"));
        assert!(report.valid);
        assert!(report.jobs_file_exists);
        assert_eq!(report.max_concurrent_jobs, 2);
        assert_eq!(report.jobs.len(), 1);
        assert_eq!(report.jobs[0].name, "daily-review");
        assert_eq!(report.jobs[0].cron, "0 0 9 * * *");
        assert!(report.jobs[0].next_run_ms.is_some());
    }

    #[test]
    fn reports_invalid_jobs_file_without_exposing_job_content() {
        let temp = tempfile::tempdir().expect("temp dir");
        let jobs_dir = temp.path().join("cron");
        std::fs::create_dir_all(&jobs_dir).expect("jobs dir");
        std::fs::write(
            jobs_dir.join("jobs.toml"),
            "[[job]]\nname = \"broken\"\ncron = \"invalid\"\nprompt = \"private prompt\"\n",
        )
        .expect("jobs file");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[cron]\njobs_dir = {:?}\n",
                jobs_dir.to_string_lossy()
            ),
        )
        .expect("config");

        let report = cron_report(&DaemonConfig::load_from(&config_path).expect("load config"));
        assert!(!report.valid);
        assert!(report.jobs.is_empty());
        assert_eq!(report.errors[0].code, "invalid_jobs_file");
        assert!(!report.errors[0].message.contains("private prompt"));
    }
}
