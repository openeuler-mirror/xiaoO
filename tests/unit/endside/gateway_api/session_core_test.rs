use crate::gateway::MemoryAutomationHealth;
use crate::status_panel::MemoryStatus;

use super::memory_status_from_health;

#[test]
fn memory_health_maps_to_operator_status() {
    assert_eq!(
        memory_status_from_health(MemoryAutomationHealth::Healthy),
        MemoryStatus::Connected
    );
    assert_eq!(
        memory_status_from_health(MemoryAutomationHealth::Degraded),
        MemoryStatus::Degraded
    );
}

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_can_initialize_before_any_agent_task() {
    use super::SessionGateway;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let gateway = SessionGateway::new();
    let session_id = uuid::Uuid::new_v4().to_string();
    let path = gateway
        .checkpoints
        .prepare(Some(&session_id), &checkpoint_config())
        .await
        .unwrap()
        .unwrap();
    assert!(gateway.session_snapshot(&session_id).await.is_none());
    let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
    stream
        .write_all(
            format!(
                "{}\n",
                serde_json::json!({"op":"init", "context":{"session_id":session_id}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .await
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["session_id"], session_id);
    assert!(
        gateway.session_snapshot(&session_id).await.is_none(),
        "init must not submit a task or create LLM history"
    );
    gateway.close_all_sessions().await;
    assert!(!path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_follows_local_remote_and_new_session_lifecycle() {
    use crate::gateway_api::{remote::RemoteRuntimeConfig, runtime::GatewayRuntime};
    let config = checkpoint_config();
    let mut gateway = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let old_id = uuid::Uuid::new_v4().to_string();
    gateway.remote = Some(RemoteRuntimeConfig {
        base_url: "http://localhost:1".into(),
        bearer_token_env: None,
    });
    gateway.remote_session_open = false;
    gateway
        .prepare_checkpoint_endpoint(&old_id, &config)
        .await
        .unwrap();
    assert!(gateway
        .session_gateway
        .checkpoints
        .driver(&old_id)
        .await
        .is_none());
    // /remote off preserves the session ID; it must now get a local endpoint.
    gateway.remote = None;
    gateway
        .prepare_checkpoint_endpoint(&old_id, &config)
        .await
        .unwrap();
    let old_driver = gateway
        .session_gateway
        .checkpoints
        .driver(&old_id)
        .await
        .unwrap();
    // This is the existing backend-release path used by /new.
    gateway.release_session_backend(&old_id).await.unwrap();
    assert!(gateway
        .session_gateway
        .checkpoints
        .driver(&old_id)
        .await
        .is_none());
    let new_id = uuid::Uuid::new_v4().to_string();
    gateway
        .prepare_checkpoint_endpoint(&new_id, &config)
        .await
        .unwrap();
    let new_driver = gateway
        .session_gateway
        .checkpoints
        .driver(&new_id)
        .await
        .unwrap();
    assert!(!std::sync::Arc::ptr_eq(&old_driver, &new_driver));
    gateway.session_gateway.close_all_sessions().await;
}

#[cfg(unix)]
fn checkpoint_config() -> xiaoo_api::runtime::checkpoint::CheckpointConfig {
    use xiaoo_api::runtime::checkpoint::{CheckpointConfig, UnixSocketConfig};
    CheckpointConfig {
        enabled: true,
        unix_socket: Some(UnixSocketConfig {
            endpoint: std::env::temp_dir()
                .join(format!("xiaoo-test-{}.sock", uuid::Uuid::new_v4())),
        }),
        ..Default::default()
    }
}
