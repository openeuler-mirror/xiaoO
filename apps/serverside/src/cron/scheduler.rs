//! Cron scheduler — per-job tokio timer loops with retry and concurrency control.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use xiaoo_shared::daemon_protocol::response::{CronJobRuntimeResponse, CronRunOutcome};

use xiaoo_shared::cron::{CronExecutionError, CronJobConfig};
use xiaoo_shared::gateway::{
    daemon_cron_principal, AppTurnRequest, GatewayEntryContext, GatewayEntryKind, SessionService,
};

// ── Public API ──────────────────────────────────────────────────

/// Manages a set of cron job timers.
pub struct CronScheduler {
    cancel_token: CancellationToken,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    jobs: HashMap<String, Arc<CronJob>>,
}

impl CronScheduler {
    /// Build a new scheduler and spawn a timer for each enabled job.
    pub fn new(
        jobs: Vec<CronJobConfig>,
        max_concurrent: usize,
        session_service: Arc<dyn SessionService>,
    ) -> Self {
        let cancel_token = CancellationToken::new();
        let limit = if max_concurrent > 0 {
            max_concurrent
        } else {
            usize::MAX
        };
        let concurrency_limiter = Arc::new(Semaphore::new(limit));

        let mut handles = Vec::new();
        let mut runtime_jobs = HashMap::new();
        let mut enabled_count = 0;
        for config in jobs {
            if !config.enabled {
                tracing::info!(job = %config.name, "cron job disabled, skipping");
                continue;
            }
            enabled_count += 1;

            let job = Arc::new(CronJob {
                config,
                session_service: session_service.clone(),
                cancel_token: cancel_token.clone(),
                concurrency_limiter: concurrency_limiter.clone(),
                next_run: Mutex::new(None),
                running: AtomicBool::new(false),
                last_result: Mutex::new(None),
                trigger_count: AtomicU64::new(0),
                success_count: AtomicU64::new(0),
                failure_count: AtomicU64::new(0),
            });

            handles.push(Self::spawn_job_timer(job.clone()));
            runtime_jobs.insert(job.config.name.clone(), job);
        }

        tracing::info!(
            enabled = enabled_count,
            total_jobs = handles.len(),
            "cron scheduler initialized"
        );

        Self {
            cancel_token,
            handles: Mutex::new(handles),
            jobs: runtime_jobs,
        }
    }

    /// Return the live state of all enabled jobs ordered by name.
    pub async fn catalog(&self) -> Vec<CronJobRuntimeResponse> {
        let mut jobs = Vec::with_capacity(self.jobs.len());
        for job in self.jobs.values() {
            jobs.push(job.runtime_response().await);
        }
        jobs.sort_by(|left, right| left.name.cmp(&right.name));
        jobs
    }

    /// Trigger an enabled job without waiting for the Agent turn to finish.
    pub async fn trigger_now(&self, name: &str) -> Result<CronJobRuntimeResponse, TriggerError> {
        let job = self.jobs.get(name).cloned().ok_or(TriggerError::NotFound)?;
        if job
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(TriggerError::AlreadyRunning);
        }
        job.trigger_count.fetch_add(1, Ordering::Relaxed);
        let execution_job = job.clone();
        let handle = tokio::spawn(async move {
            execute_job(execution_job).await;
        });
        self.handles.lock().await.push(handle);
        Ok(job.runtime_response().await)
    }

    /// Cancel all timers and wait for them to exit gracefully.
    pub async fn stop(&self) {
        tracing::info!("stopping cron scheduler...");
        self.cancel_token.cancel();

        let handles = {
            let mut guard = self.handles.lock().await;
            std::mem::take(&mut *guard)
        };

        for handle in handles {
            let _ = handle.await;
        }

        tracing::info!("cron scheduler stopped");
    }
}

/// Reason a manual Cron trigger could not be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerError {
    /// The active scheduler does not contain this enabled job.
    NotFound,
    /// A previous scheduled or manual execution is still active.
    AlreadyRunning,
}

// ── Internal job runtime ────────────────────────────────────────

struct CronJob {
    config: CronJobConfig,
    session_service: Arc<dyn SessionService>,
    cancel_token: CancellationToken,
    concurrency_limiter: Arc<Semaphore>,
    next_run: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
    running: AtomicBool,
    last_result: Mutex<Option<CronLastResult>>,
    /// Number of times this job was triggered.
    trigger_count: AtomicU64,
    /// Number of successful executions.
    success_count: AtomicU64,
    /// Number of executions that failed permanently (after all retries exhausted).
    failure_count: AtomicU64,
}

struct CronLastResult {
    started_at_ms: u64,
    completed_at_ms: u64,
    outcome: CronRunOutcome,
    session_id: Option<String>,
    reply: Option<String>,
    error: Option<String>,
    total_tokens: Option<u64>,
    duration_ms: u64,
}

impl CronJob {
    async fn runtime_response(&self) -> CronJobRuntimeResponse {
        let next_run_ms = self
            .next_run
            .lock()
            .await
            .as_ref()
            .and_then(|time| u64::try_from(time.timestamp_millis()).ok());
        let last = self.last_result.lock().await;
        CronJobRuntimeResponse {
            name: self.config.name.clone(),
            running: self.running.load(Ordering::Acquire),
            next_run_ms,
            trigger_count: self.trigger_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            failure_count: self.failure_count.load(Ordering::Relaxed),
            last_started_at_ms: last.as_ref().map(|result| result.started_at_ms),
            last_completed_at_ms: last.as_ref().map(|result| result.completed_at_ms),
            last_outcome: last.as_ref().map(|result| result.outcome.clone()),
            last_session_id: last.as_ref().and_then(|result| result.session_id.clone()),
            last_reply: last.as_ref().and_then(|result| result.reply.clone()),
            last_error: last.as_ref().and_then(|result| result.error.clone()),
            last_total_tokens: last.as_ref().and_then(|result| result.total_tokens),
            last_duration_ms: last.as_ref().map(|result| result.duration_ms),
        }
    }
}

impl CronScheduler {
    fn spawn_job_timer(job: Arc<CronJob>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tracing::info!(
                job = %job.config.name,
                cron = %job.config.cron,
                "cron job timer started"
            );

            loop {
                // 1. Compute next trigger time
                let now = chrono::Utc::now();
                let Some(next) = job.config.cron.next_after(now) else {
                    tracing::error!(
                        job = %job.config.name,
                        "cron expression has no future match, stopping timer"
                    );
                    break;
                };

                *job.next_run.lock().await = Some(next);

                let wait = match (next - now).to_std() {
                    Ok(d) if d > Duration::ZERO => d,
                    _ => Duration::ZERO,
                };

                tracing::info!(
                    job = %job.config.name,
                    next_run = %next.format("%Y-%m-%dT%H:%M:%SZ"),
                    wait_secs = wait.as_secs(),
                    "waiting for next run"
                );

                // 2. Wait until trigger time or cancellation
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = job.cancel_token.cancelled() => {
                        tracing::info!(job = %job.config.name, "cancelled");
                        break;
                    }
                }

                // 3. Skip an overlapping manual execution.
                if job
                    .running
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    tracing::warn!(job = %job.config.name, "cron trigger skipped while job is running");
                    continue;
                }
                *job.next_run.lock().await = None;
                job.trigger_count.fetch_add(1, Ordering::Relaxed);
                tracing::info!(
                    job = %job.config.name,
                    "cron triggered, acquiring concurrency permit"
                );

                execute_job(job.clone()).await;
            }

            tracing::info!(job = %job.config.name, "cron job timer stopped");
        })
    }
}

// ── Execution helpers ───────────────────────────────────────────

/// Outcome of a job execution attempt.
enum ExecutionOutcome {
    /// Job completed successfully.
    Success(JobRunResult),
    /// Job failed permanently after all retries.
    Failed(String),
    /// Job was cancelled during execution or retry backoff.
    Cancelled,
}

async fn execute_job(job: Arc<CronJob>) {
    let started_at_ms = unix_time_ms();
    let started = std::time::Instant::now();
    let permit = tokio::select! {
        permit = job.concurrency_limiter.acquire() => permit.ok(),
        _ = job.cancel_token.cancelled() => None,
    };
    let outcome = match permit {
        Some(permit) => {
            let outcome = execute_job_with_retry(&job).await;
            drop(permit);
            outcome
        }
        None => ExecutionOutcome::Cancelled,
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    let result = match outcome {
        ExecutionOutcome::Success(result) => {
            job.success_count.fetch_add(1, Ordering::Relaxed);
            CronLastResult {
                started_at_ms,
                completed_at_ms: unix_time_ms(),
                outcome: CronRunOutcome::Succeeded,
                session_id: Some(result.session_id),
                reply: Some(truncate_reply(result.reply)),
                error: None,
                total_tokens: Some(result.total_tokens),
                duration_ms,
            }
        }
        ExecutionOutcome::Failed(error) => {
            job.failure_count.fetch_add(1, Ordering::Relaxed);
            CronLastResult {
                started_at_ms,
                completed_at_ms: unix_time_ms(),
                outcome: CronRunOutcome::Failed,
                session_id: None,
                reply: None,
                error: Some(error),
                total_tokens: None,
                duration_ms,
            }
        }
        ExecutionOutcome::Cancelled => CronLastResult {
            started_at_ms,
            completed_at_ms: unix_time_ms(),
            outcome: CronRunOutcome::Cancelled,
            session_id: None,
            reply: None,
            error: None,
            total_tokens: None,
            duration_ms,
        },
    };
    *job.last_result.lock().await = Some(result);
    job.running.store(false, Ordering::Release);
}

fn unix_time_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

fn truncate_reply(reply: String) -> String {
    const MAX_CHARS: usize = 4_000;
    let mut chars = reply.chars();
    let truncated: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

async fn execute_job_with_retry(job: &CronJob) -> ExecutionOutcome {
    let max_attempts = job.config.max_retries.saturating_add(1);

    for attempt in 1..=max_attempts {
        match execute_job_once(job).await {
            Ok(result) => {
                tracing::info!(
                    job = %job.config.name,
                    attempt,
                    session_id = %result.session_id,
                    reply = %result.reply,
                    total_tokens = %result.total_tokens,
                    duration_ms = %result.duration_ms,
                    "cron job completed"
                );
                return ExecutionOutcome::Success(result);
            }
            Err(error) if attempt < max_attempts => {
                tracing::warn!(
                    job = %job.config.name,
                    attempt,
                    max_attempts,
                    error = %error,
                    retry_delay_secs = job.config.retry_delay_secs,
                    "cron job failed, will retry"
                );

                // Wait before retry, but allow cancellation
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(
                        job.config.retry_delay_secs,
                    )) => {}
                    _ = job.cancel_token.cancelled() => {
                        tracing::info!(
                            job = %job.config.name,
                            "cancelled during retry backoff"
                        );
                        return ExecutionOutcome::Cancelled;
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    job = %job.config.name,
                    attempt,
                    max_attempts,
                    error = %error,
                    "cron job permanently failed"
                );
                return ExecutionOutcome::Failed(error.to_string());
            }
        }
    }

    // This should not be reached, but return Failed as fallback
    ExecutionOutcome::Failed("cron job exhausted retries without a result".to_string())
}

struct JobRunResult {
    reply: String,
    session_id: String,
    total_tokens: u64,
    duration_ms: u64,
}

async fn execute_job_once(job: &CronJob) -> Result<JobRunResult, CronExecutionError> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let session_id = format!("cron-{}-{}", job.config.name, ts);
    let conversation_id = format!("cron-{}-conv", job.config.name);

    let request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext {
            kind: Some(GatewayEntryKind::ScheduledJob),
            runtime_profile_id: job.config.agent_role.clone(),
            ..Default::default()
        },
        channel: None,
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        conversation_id,
        sender_id: format!("cron/{}", job.config.name),
        text: job.config.prompt.clone(),
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: vec![],
        reasoning_effort: None,
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        client_id: Some(daemon_cron_principal()),
    };

    tracing::info!(
        job = %job.config.name,
        session_id = %session_id,
        prompt_len = job.config.prompt.len(),
        "submitting turn to session service"
    );

    let start = std::time::Instant::now();

    let result = tokio::time::timeout(
        Duration::from_secs(job.config.timeout_secs),
        job.session_service.run_turn(request),
    )
    .await
    .map_err(|_| CronExecutionError::Timeout {
        job_name: job.config.name.clone(),
        timeout_secs: job.config.timeout_secs,
    })?
    .map_err(|e| CronExecutionError::Session {
        job_name: job.config.name.clone(),
        error: e.to_string(),
    })?;

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(JobRunResult {
        session_id,
        reply: result.visible_reply.clone(),
        total_tokens: result.total_tokens,
        duration_ms,
    })
}
