//! Backend 状态 / 端点的显示文案。
//!
//! 原属 serverside `httpserver/dashboard.rs` 的纯展示函数，下沉到 shared：
//! dashboard 从 shared API（`BackendInfo`）拿到状态值后直接传参即可，
//! 无需在应用层命名 `BackendLifecycleState` / `BackendEndpoint`。
//!
//! 若 serverside 其余测试仍需构造这两个枚举做 fixture，再补配置词汇
//! 再导出；否则不导出，避免暴露无消费者的类型。

use agent_contracts::backend::{BackendEndpoint, BackendLifecycleState};

/// Backend 生命周期状态的展示文案。
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

/// Backend 端点的展示文案；`None` 端点返回 `None`。
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
