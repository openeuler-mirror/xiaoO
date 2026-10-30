//! CLI task deadline and bounded session shutdown.
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use xiaoo_shared::gateway::{
    AppTurnRequest, AppTurnResult, SessionControlPlane, SessionService, SessionServiceError,
};

pub(super) const DEFAULT_TASK_TIMEOUT_SECS: u64 = 600;
pub(super) const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const CANCEL_RPC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(super) struct TaskDeadline {
    expires_at: Instant,
    timeout_secs: u64,
}

impl TaskDeadline {
    pub(super) fn new(timeout_secs: u64) -> Result<Self, String> {
        if timeout_secs == 0 {
            return Err("--timeout must be greater than zero".into());
        }
        let expires_at = Instant::now()
            .checked_add(Duration::from_secs(timeout_secs))
            .ok_or_else(|| "--timeout is too large".to_string())?;
        Ok(Self {
            expires_at,
            timeout_secs,
        })
    }

    pub(super) fn timeout_message(self) -> String {
        format!(
            "CLI task deadline exceeded after {} seconds",
            self.timeout_secs
        )
    }

    pub(super) async fn run<F: Future>(self, future: F) -> Result<F::Output, String> {
        // Do not start another phase once the overall budget has expired.
        if Instant::now() >= self.expires_at {
            return Err(self.timeout_message());
        }
        tokio::time::timeout_at(self.expires_at, future)
            .await
            .map_err(|_| self.timeout_message())
    }
}

pub(super) async fn run_local_turn(
    deadline: TaskDeadline,
    service: &dyn SessionService,
    control: &dyn SessionControlPlane,
    request: AppTurnRequest,
    cancel: &CancellationToken,
) -> Result<AppTurnResult, String> {
    let session_id = request.session_id.clone();
    let result = match deadline.run(service.run_turn(request)).await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(error) => {
            // Dropping run_turn alone does not stop its session actor.
            cancel.cancel();
            // Shell tools finish before observing turn cancellation. Stop
            // CLI-owned process groups so session shutdown can drain them.
            drop(xiaoo_shared::backend::ProcessGroupCleanupGuard);
            Err(error)
        }
    };
    let close = match tokio::time::timeout(
        SESSION_CLOSE_TIMEOUT,
        control.force_close_session(&session_id),
    )
    .await
    {
        Ok(Ok(_)) | Ok(Err(SessionServiceError::SessionNotFound { .. })) => Ok(()),
        Ok(Err(error)) => Err(format!("failed to close session: {error}")),
        Err(_) => Err(format!(
            "session close timed out after {} seconds",
            SESSION_CLOSE_TIMEOUT.as_secs()
        )),
    };
    match (result, close) {
        (result, Ok(())) => result,
        (Err(error), Err(close_error)) => Err(format!("{error}; {close_error}")),
        (Ok(_), Err(error)) => Err(error),
    }
}
