use crate::backend::{
    BackendCheckoutRequest, BackendEnsureSessionRequest, BackendError, BackendLease,
    BackendManager, GatewayBackendConfig,
};
use crate::gateway::{ResolvedSessionRuntime, SessionRecord, SessionServiceError};
use agent_contracts::backend::{BackendResourceLimits, OperationBackendBuildError};
use serde_json::Value;
use std::sync::Arc;

pub(super) async fn lease_session_backend(
    backend_manager: &BackendManager,
    session: &SessionRecord,
    resolved: &ResolvedSessionRuntime,
    session_store: Arc<dyn crate::gateway::SessionStore>,
) -> Result<BackendLease, SessionServiceError> {
    // Resume a session whose sandbox was evicted: check out a new sandbox from
    // the eviction checkpoint before running the turn.
    if session.status == crate::gateway::SessionLifecycleStatus::Paused {
        if let Some(checkpoint) = session.paused_backend_checkpoint.clone() {
            return checkout_backend_with_eviction(
                backend_manager,
                &session.session_id,
                checkpoint,
                session_store,
                Value::Null,
                &CheckoutEvictionContext::resume(),
            )
            .await;
        }
        let is_local_mcp = session.entry.kind == Some(crate::gateway::GatewayEntryKind::Mcp)
            && resolved
                .operation_backend
                .as_ref()
                .map(|config| config.kind == "local")
                .unwrap_or(true);
        if !is_local_mcp {
            return Err(SessionServiceError::SessionBusy {
                session_id: session.session_id.clone(),
                message: "session is paused and has no eviction checkpoint to resume from"
                    .to_string(),
            });
        }
    }

    match backend_manager
        .ensure_session_backend(
            BackendEnsureSessionRequest {
                config: resolved.operation_backend.clone(),
                workspace_root: resolved.backend_workspace_root.clone(),
                session_id: session.session_id.clone(),
                e2b_bootstrap: resolved.e2b_bootstrap.clone(),
            },
            session_store,
        )
        .await
    {
        Ok(lease) => Ok(lease),
        Err(OperationBackendBuildError::ResourceLimitExceeded { message }) => {
            Err(SessionServiceError::SessionBusy {
                session_id: session.session_id.clone(),
                message,
            })
        }
        Err(error) => Err(SessionServiceError::RuntimeBuild {
            message: format!("failed to lease session backend: {error}"),
        }),
    }
}

/// Labels injected into the error messages produced by
/// [`checkout_backend_with_eviction`], so that different callers (resume vs.
/// checkpoint checkout) can be distinguished in surfaced errors.
pub(super) struct CheckoutEvictionContext {
    /// Context used when `checkout_backend` itself returns a non-recoverable
    /// error.
    pub checkout: &'static str,
    /// Context used when `try_evict_if_needed` returns a non-recoverable
    /// error.
    pub eviction: &'static str,
    /// Prefix used when leasing the just-checked-out backend fails.
    pub lease: &'static str,
    /// Message used when the eviction retry budget is exhausted.
    pub exhausted: &'static str,
}

impl CheckoutEvictionContext {
    fn resume() -> Self {
        Self {
            checkout: "resume checkout",
            eviction: "resume checkout eviction",
            lease: "failed to lease resumed backend",
            exhausted: "resume failed: sandbox limit reached and no idle sandbox to evict",
        }
    }

    pub(super) fn runtime_checkout() -> Self {
        Self {
            checkout: "checkout runtime backend",
            eviction: "checkout runtime backend eviction",
            lease: "failed to lease checked out backend",
            exhausted: "checkout runtime backend failed: sandbox limit reached and no idle sandbox to evict",
        }
    }
}

/// Check out a backend from `checkpoint` for `session_id`, retrying with
/// `try_evict_if_needed` when the sandbox pool is full.
///
/// This is the shared recovery path for both the paused-session resume flow
/// and the checkpoint-checkout flow. When `checkout_backend` returns
/// [`BackendError::NeedsEviction`], we ask the backend manager to evict (or
/// mark for eviction) an idle sandbox and retry, up to
/// `max_eviction_attempts` times. Only non-`NeedsEviction` errors — or
/// exhaustion of the retry budget — surface to the caller.
pub(super) async fn checkout_backend_with_eviction(
    backend_manager: &BackendManager,
    session_id: &str,
    checkpoint: crate::backend::BackendCheckpointRef,
    session_store: Arc<dyn crate::gateway::SessionStore>,
    metadata: Value,
    context: &CheckoutEvictionContext,
) -> Result<BackendLease, SessionServiceError> {
    let sandbox_key = BackendManager::sandbox_key_from_config(&GatewayBackendConfig::new(
        checkpoint.provider.clone(),
        checkpoint.provider_options.clone(),
    ));
    let max_attempts = backend_manager.limits().max_eviction_attempts.max(1);

    // Track the remote backend (if any) that *this* call marked for
    // eviction via `try_evict_if_needed`. If we exhaust the retry budget
    // without securing a backend, we clear the mark so the next attempt
    // starts from a clean slate and re-targets the earliest idle sandbox
    // rather than skipping it (a lingering mark makes `is_evictable`
    // return false for the marked backend).
    let mut marked_backend_id: Option<String> = None;

    for _ in 0..max_attempts {
        match backend_manager
            .checkout_backend(BackendCheckoutRequest {
                checkpoint: checkpoint.clone(),
                backend_id: None,
                session_id: Some(session_id.to_string()),
                timeout: None,
                metadata: metadata.clone(),
                resource_limits: BackendResourceLimits::default(),
                options: None,
            })
            .await
        {
            Ok(_) => {
                let lease = backend_manager
                    .lease_bound_session(session_id)
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!("{}: {error}", context.lease),
                    })?;
                return Ok(lease);
            }
            Err(BackendError::NeedsEviction) => {
                match backend_manager
                    .try_evict_if_needed(
                        &sandbox_key,
                        &checkpoint.provider_options,
                        session_store.clone(),
                    )
                    .await
                {
                    Ok(None) => {
                        // A sandbox was evicted directly (own or orphan).
                        // Clear any mark we previously set: we no longer
                        // need a remote eviction, and leaving the mark
                        // would cause an unnecessary eviction of an idle
                        // sandbox.
                        if let Some(id) = marked_backend_id.take() {
                            backend_manager.clear_pending_eviction(&id).await;
                        }
                        continue;
                    }
                    Ok(Some(id)) => {
                        // A remote sandbox was marked for eviction. Record
                        // it so we can roll back on give-up, then wait for
                        // its owner to evict it before retrying.
                        marked_backend_id = Some(id);
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    Err(BackendError::NeedsEviction) => {
                        // Either no evictable sandbox was found, or another
                        // backend is already marked for eviction. Wait
                        // briefly for the mark to be resolved and retry.
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    Err(error) => {
                        return Err(map_backend_error(context.eviction, session_id, error));
                    }
                }
            }
            Err(error) => return Err(map_backend_error(context.checkout, session_id, error)),
        }
    }

    // Retry budget exhausted. Roll back any mark this call set so the final
    // state is equivalent to "no deletable sandbox found, just an error".
    if let Some(id) = marked_backend_id.take() {
        backend_manager.clear_pending_eviction(&id).await;
    }

    Err(SessionServiceError::SessionBusy {
        session_id: session_id.to_string(),
        message: context.exhausted.to_string(),
    })
}

fn map_backend_error(context: &str, session_id: &str, error: BackendError) -> SessionServiceError {
    match error {
        BackendError::ResourceLimitExceeded { message } => SessionServiceError::SessionBusy {
            session_id: session_id.to_string(),
            message,
        },
        error => SessionServiceError::RuntimeBuild {
            message: format!("{context}: {error}"),
        },
    }
}

pub(super) fn sync_session_backend_instance(
    session: &mut SessionRecord,
    lease: &BackendLease,
) -> bool {
    let lease_instance = lease.instance();
    if session.backend_instance.as_ref() == Some(&lease_instance) {
        return false;
    }
    session.backend_instance = Some(lease_instance);
    true
}
