//! Cron scheduler — per-job tokio timer loops with retry and concurrency control.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use agent_types::cron::{CronExecutionError, CronJobConfig};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use xiaoo_shared::gateway::{
    AppTurnRequest, GatewayEntryContext, GatewayEntryKind, SessionService,
};

// ── Public API ──────────────────────────────────────────────────

/// Manages a set of cron job timers.
pub struct CronScheduler {
    cancel_token: CancellationToken,
    #[allow(dead_code)]
    concurrency_limiter: Arc<Semaphore>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
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
                last_run: Mutex::new(None),
                next_run: Mutex::new(None),
                trigger_count: AtomicU64::new(0),
                success_count: AtomicU64::new(0),
                failure_count: AtomicU64::new(0),
            });

            handles.push(Self::spawn_job_timer(job));
        }

        tracing::info!(
            enabled = enabled_count,
            total_jobs = handles.len(),
            "cron scheduler initialized"
        );

        Self {
            cancel_token,
            concurrency_limiter,
            handles: Mutex::new(handles),
        }
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

// ── Internal job runtime ────────────────────────────────────────

struct CronJob {
    config: CronJobConfig,
    session_service: Arc<dyn SessionService>,
    cancel_token: CancellationToken,
    #[allow(dead_code)]
    concurrency_limiter: Arc<Semaphore>,
    last_run: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
    next_run: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
    /// Number of times this job was triggered.
    trigger_count: AtomicU64,
    /// Number of successful executions.
    success_count: AtomicU64,
    /// Number of executions that failed permanently (after all retries exhausted).
    failure_count: AtomicU64,
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

                // 3. Record trigger and log
                job.trigger_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::info!(
                    job = %job.config.name,
                    "cron triggered, acquiring concurrency permit"
                );

                // 4. Acquire concurrency permit
                let permit = match job.concurrency_limiter.acquire().await {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::info!(job = %job.config.name, "semaphore closed, exiting");
                        break;
                    }
                };

                // 5. Execute with retry (permit released when dropped)
                let result = execute_job_with_retry(&job).await;
                drop(permit); // Explicitly release permit before updating stats

                // 6. Update stats
                *job.last_run.lock().await = Some(chrono::Utc::now());
                match result {
                    ExecutionOutcome::Success => {
                        job.success_count
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    ExecutionOutcome::Failed => {
                        job.failure_count
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    ExecutionOutcome::Cancelled => {
                        // Don't count as success or failure
                    }
                }
            }

            tracing::info!(job = %job.config.name, "cron job timer stopped");
        })
    }
}

// ── Execution helpers ───────────────────────────────────────────

/// Outcome of a job execution attempt.
enum ExecutionOutcome {
    /// Job completed successfully.
    Success,
    /// Job failed permanently after all retries.
    Failed,
    /// Job was cancelled during execution or retry backoff.
    Cancelled,
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
                return ExecutionOutcome::Success;
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
                return ExecutionOutcome::Failed;
            }
        }
    }

    // This should not be reached, but return Failed as fallback
    ExecutionOutcome::Failed
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
        reasoning_effort: agent_types::ReasoningEffort::Off,
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
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
