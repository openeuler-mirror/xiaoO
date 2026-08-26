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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_label_covers_all_variants() {
        let all = [
            BackendLifecycleState::Unknown,
            BackendLifecycleState::Creating,
            BackendLifecycleState::Active,
            BackendLifecycleState::Pausing,
            BackendLifecycleState::Paused,
            BackendLifecycleState::Loading,
            BackendLifecycleState::Deleting,
            BackendLifecycleState::Deleted,
            BackendLifecycleState::Failed,
        ];
        for state in all {
            assert!(!backend_state_label(state).is_empty());
        }
    }

    #[test]
    fn endpoint_label_none() {
        assert!(backend_endpoint_str(None).is_none());
    }

    #[test]
    fn endpoint_label_local() {
        assert_eq!(
            backend_endpoint_str(Some(BackendEndpoint::Local)).as_deref(),
            Some("local")
        );
    }

    #[test]
    fn endpoint_label_tcp() {
        let s = backend_endpoint_str(Some(BackendEndpoint::Tcp {
            host: "127.0.0.1".to_string(),
            port: 8080,
        }));
        assert_eq!(s.as_deref(), Some("tcp://127.0.0.1:8080"));
    }

    #[test]
    fn endpoint_label_unix() {
        let s = backend_endpoint_str(Some(BackendEndpoint::UnixSocket {
            path: "/run/x.sock".to_string(),
        }));
        assert_eq!(s.as_deref(), Some("unix:/run/x.sock"));
    }

    #[test]
    fn endpoint_label_provider_handle() {
        let s = backend_endpoint_str(Some(BackendEndpoint::ProviderHandle {
            value: serde_json::Value::String("abc".to_string()),
        }));
        assert_eq!(s.as_deref(), Some("provider:\"abc\""));
    }
}
