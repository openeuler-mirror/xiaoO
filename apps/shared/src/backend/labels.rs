//! Display labels for backend state / endpoints.
//!
//! The dashboard gets the state value via the shared API (`BackendInfo`)
//! and passes it straight in here — the app layer never has to name
//! `BackendLifecycleState` / `BackendEndpoint`.
//!
//! The enums are deliberately not re-exported: re-export only if some
//! serverside test actually needs to construct them as fixtures; exposing
//! types without consumers is the thing this module exists to avoid.

use agent_contracts::backend::{BackendEndpoint, BackendLifecycleState};

pub fn backend_state_label(state: BackendLifecycleState) -> &'static str {
    match state {
        BackendLifecycleState::Unknown => "unknown",
        BackendLifecycleState::Creating => "creating",
        BackendLifecycleState::Active => "active",
        BackendLifecycleState::Pausing => "pausing",
        BackendLifecycleState::Paused => "paused",
        BackendLifecycleState::Loading => "loading",
        BackendLifecycleState::Deleting => "deleting",
        BackendLifecycleState::Deleted => "deleted",
        BackendLifecycleState::Failed => "failed",
    }
}

/// Returns `None` when `endpoint` is `None`.
pub fn backend_endpoint_str(endpoint: Option<BackendEndpoint>) -> Option<String> {
    endpoint.map(|e| match e {
        BackendEndpoint::Local => "local".to_string(),
        BackendEndpoint::Tcp { host, port } => format!("tcp://{host}:{port}"),
        BackendEndpoint::UnixSocket { path } => format!("unix:{path}"),
        BackendEndpoint::ProviderHandle { value } => format!("provider:{value}"),
    })
}

/// Return only the endpoint transport class, without Provider handles or addresses.
pub fn backend_endpoint_kind(endpoint: Option<BackendEndpoint>) -> Option<&'static str> {
    endpoint.map(|endpoint| match endpoint {
        BackendEndpoint::Local => "local",
        BackendEndpoint::Tcp { .. } => "tcp",
        BackendEndpoint::UnixSocket { .. } => "unix_socket",
        BackendEndpoint::ProviderHandle { .. } => "provider_handle",
    })
}

#[cfg(test)]
#[path = "../../../../tests/unit/shared/backend/labels_test.rs"]
mod tests;
