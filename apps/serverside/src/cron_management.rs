use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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

#[derive(Debug, Clone, Deserialize)]
pub struct CronJobsDraft {
    pub jobs: Vec<CronJobDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobDraft {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub cron: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    pub timeout_secs: u64,
    pub enabled: bool,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
}

#[derive(Debug, Serialize)]
struct CronJobsDocument {
    #[serde(rename = "job")]
    jobs: Vec<CronJobDraft>,
}

#[derive(Debug, Serialize)]
pub struct CronRenderReport {
    pub schema_version: u32,
    pub valid: bool,
    pub content: Option<String>,
    pub errors: Vec<CronIssue>,
}

pub fn render_cron_jobs(draft: CronJobsDraft) -> CronRenderReport {
    let mut errors = Vec::new();
    let mut names = HashSet::new();
    for (index, job) in draft.jobs.iter().enumerate() {
        let path = format!("cron.jobs[{index}]");
        if job.name.trim().is_empty() {
            issue(
                &mut errors,
                &format!("{path}.name"),
                "required",
                "任务名称不能为空",
            );
        } else if !names.insert(job.name.trim()) {
            issue(
                &mut errors,
                &format!("{path}.name"),
                "duplicate",
                "任务名称不能重复",
            );
        }
        if let Err(error) = xiaoo_shared::cron::validate_cron_expr(&job.cron) {
            issue(&mut errors, &format!("{path}.cron"), "invalid_cron", error);
        }
        if job.prompt.trim().is_empty() {
            issue(
                &mut errors,
                &format!("{path}.prompt"),
                "required",
                "任务 Prompt 不能为空",
            );
        }
        if job.timeout_secs == 0 {
            issue(
                &mut errors,
                &format!("{path}.timeout_secs"),
                "out_of_range",
                "任务超时必须大于 0",
            );
        }
    }
    if !errors.is_empty() {
        return CronRenderReport {
            schema_version: 1,
            valid: false,
            content: None,
            errors,
        };
    }

    let document = CronJobsDocument { jobs: draft.jobs };
    match toml::to_string_pretty(&document) {
        Ok(content) => CronRenderReport {
            schema_version: 1,
            valid: true,
            content: Some(content),
            errors: Vec::new(),
        },
        Err(error) => CronRenderReport {
            schema_version: 1,
            valid: false,
            content: None,
            errors: vec![CronIssue {
                path: "cron.jobs".to_string(),
                code: "serialization_failed".to_string(),
                message: error.to_string(),
            }],
        },
    }
}

fn issue(errors: &mut Vec<CronIssue>, path: &str, code: &str, message: impl Into<String>) {
    errors.push(CronIssue {
        path: path.to_string(),
        code: code.to_string(),
        message: message.into(),
    });
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
#[path = "../../../tests/unit/serverside/cron_management_test.rs"]
mod tests;
