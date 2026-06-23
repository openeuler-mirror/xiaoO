use crate::backend::{BackendEnsureSessionRequest, BackendLease, BackendManager};
use crate::gateway::{ResolvedSessionRuntime, SessionRecord, SessionServiceError};
use agent_contracts::backend::OperationBackendBuildError;

pub(super) async fn lease_session_backend(
    backend_manager: &BackendManager,
    session: &SessionRecord,
    resolved: &ResolvedSessionRuntime,
) -> Result<BackendLease, SessionServiceError> {
    match backend_manager
        .ensure_session_backend(BackendEnsureSessionRequest {
            config: resolved.operation_backend.clone(),
            workspace_root: resolved.descriptor.workspace_root.clone(),
            session_id: session.session_id.clone(),
        })
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
