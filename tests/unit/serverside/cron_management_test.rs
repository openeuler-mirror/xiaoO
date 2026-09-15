use super::{cron_report, render_cron_jobs, CronJobDraft, CronJobsDraft};
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

#[test]
fn renders_validated_jobs_document() {
    let report = render_cron_jobs(CronJobsDraft {
        jobs: vec![CronJobDraft {
            name: "daily-review".to_string(),
            description: None,
            cron: "0 9 * * *".to_string(),
            prompt: "Review repository".to_string(),
            agent_role: Some("plan".to_string()),
            timeout_secs: 120,
            enabled: true,
            max_retries: 1,
            retry_delay_secs: 60,
        }],
    });
    assert!(report.valid);
    let content = report.content.expect("rendered content");
    assert!(content.contains("[[job]]"));
    assert!(content.contains("name = \"daily-review\""));
}

#[test]
fn rejects_duplicate_names_invalid_cron_and_empty_prompt() {
    let invalid = |cron: &str| CronJobDraft {
        name: "duplicate".to_string(),
        description: None,
        cron: cron.to_string(),
        prompt: "".to_string(),
        agent_role: None,
        timeout_secs: 0,
        enabled: true,
        max_retries: 0,
        retry_delay_secs: 60,
    };
    let report = render_cron_jobs(CronJobsDraft {
        jobs: vec![invalid("invalid"), invalid("0 9 * * *")],
    });
    assert!(!report.valid);
    assert!(report.content.is_none());
    assert!(report.errors.iter().any(|error| error.code == "duplicate"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "invalid_cron"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.path.ends_with("prompt")));
}
