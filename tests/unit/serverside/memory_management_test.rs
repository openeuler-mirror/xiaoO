use super::{memory_automation_report, memory_queue_action_report, MemoryQueueAction};
use crate::daemon_config::DaemonConfig;

#[tokio::test]
async fn disabled_memory_report_does_not_connect() {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[memory_automation]\nenabled = false\nserver = \"missing\"\n",
    )
    .expect("write config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");

    let report = memory_automation_report(&config).await;

    assert!(report.success);
    assert!(!report.connected);
    assert_eq!(report.pending_ingests, Some(0));
    assert_eq!(report.failed_ingests, Some(0));
    assert_eq!(report.error_code, None);
}

#[tokio::test]
async fn missing_enabled_memory_server_returns_safe_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[memory_automation]\nenabled = true\nserver = \"missing\"\n",
    )
    .expect("write config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");

    let report = memory_automation_report(&config).await;

    assert!(!report.success);
    assert!(!report.connected);
    assert_eq!(report.error_code, Some("invalid_configuration"));
    assert!(!report.error.expect("safe error").contains("missing"));
}

#[tokio::test]
async fn clears_failed_queue_without_connecting_memory_server() {
    let temp = tempfile::tempdir().expect("temp dir");
    let queue_path = temp.path().join("memory-queue.jsonl");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &queue_path,
        "{\"message_id\":\"message-1\",\"conversation_id\":\"conversation-1\",\"sender_id\":\"sender-1\",\"agent_role\":\"defaultagent\",\"timestamp_ms\":1,\"user_text\":\"secret\",\"assistant_text\":\"secret\",\"retries\":2,\"next_attempt_ms\":18446744073709551615,\"failed\":true}\n",
    )
    .expect("write queue");
    std::fs::write(
        &config_path,
        format!(
            "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[memory_automation]\nqueue_path = {:?}\nmax_retries = 1\n",
            queue_path.display().to_string()
        ),
    )
    .expect("write config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");

    let status = memory_queue_action_report(&config, MemoryQueueAction::Status).await;
    assert_eq!(status.failed_ingests, Some(1));
    let cleared = memory_queue_action_report(&config, MemoryQueueAction::ClearFailed).await;
    assert!(cleared.success);
    assert_eq!(cleared.affected_ingests, 1);
    assert_eq!(cleared.failed_ingests, Some(0));
    assert!(!serde_json::to_string(&cleared)
        .expect("serialize")
        .contains("secret"));
}
