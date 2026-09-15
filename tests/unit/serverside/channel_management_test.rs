use super::{channel_error_kind, channel_report, ChannelManager};
use crate::channels::{ChannelError, FeishuConfig, FeishuEventTransport};
use crate::daemon_config::{AppConfig, DaemonConfig};
use std::path::PathBuf;

fn config(content: &str) -> DaemonConfig {
    DaemonConfig {
        app: toml::from_str::<AppConfig>(content).expect("config"),
        config_path: PathBuf::from("/tmp/config.toml"),
    }
}

#[test]
fn reports_defaults_without_creating_channel_runtimes() {
    let config = config("[llm]\nprovider='local'\nmodel='test'\n");
    let report = channel_report(&config);
    assert_eq!(report.interaction_timeout_secs, 600);
    assert_eq!(report.channels.len(), 2);
    assert!(report.channels.iter().all(|channel| channel.valid));
    assert!(report.channels.iter().all(|channel| !channel.configured));
}

#[test]
fn reports_transport_defaults_and_missing_secrets_without_values() {
    let config = config(
        "[llm]\nprovider='local'\nmodel='test'\n\
             [channels]\ninteraction_timeout_secs=30\n\
             [channels.feishu]\nenabled=true\ntransport='websocket'\napp_id='cli_a'\napp_secret_env='XIAOO_TEST_MISSING_FEISHU'\n\
             [channels.telegram]\nenabled=true\ntransport='polling'\nbot_token_env='XIAOO_TEST_MISSING_TELEGRAM'\n",
    );
    let report = channel_report(&config);
    let feishu = &report.channels[0];
    assert_eq!(feishu.transport, "websocket");
    assert!(feishu.webhook_path.is_none());
    assert!(!feishu.valid);
    assert_eq!(feishu.errors[0].code, "missing_secret");
    let telegram = &report.channels[1];
    assert_eq!(telegram.transport, "polling");
    assert_eq!(telegram.polling_timeout_secs, Some(50));
    assert_eq!(telegram.polling_limit, Some(100));
    assert!(!telegram.valid);
    let json = serde_json::to_string(&report).expect("report JSON");
    assert!(!json.contains("secret-value"));
}

#[test]
fn runtime_catalog_tracks_only_loaded_channels() {
    let manager = ChannelManager::new(
        Some(FeishuConfig {
            channel_instance_id: None,
            base_url: "https://open.feishu.cn".to_string(),
            app_id: "app-id".to_string(),
            app_secret_env: "FEISHU_SECRET".to_string(),
            event_transport: FeishuEventTransport::Websocket,
            verification_token: None,
            parse_file_messages: false,
            max_file_download_bytes: 0,
            max_file_text_chars: 0,
        }),
        None,
    );
    manager.record_received("feishu");
    manager.record_failure("feishu", "gateway");
    manager.record_received("unknown");

    let catalog = manager.catalog();
    assert_eq!(catalog.channels.len(), 1);
    let channel = &catalog.channels[0];
    assert_eq!(channel.id, "feishu");
    assert_eq!(channel.transport, "websocket");
    assert_eq!(channel.received_count, 1);
    assert_eq!(channel.failure_count, 1);
    assert_eq!(channel.last_error_kind.as_deref(), Some("gateway"));
    assert!(channel.last_received_at_ms.is_some());
    assert!(channel.last_completed_at_ms.is_some());

    manager.record_success("feishu");
    let channel = &manager.catalog().channels[0];
    assert_eq!(channel.success_count, 1);
    assert!(channel.last_error_kind.is_none());
}

#[test]
fn classifies_channel_errors_without_exposing_messages() {
    let error = ChannelError::Authentication {
        message: "sensitive provider response".to_string(),
    };
    assert_eq!(channel_error_kind(&error), "authentication");
    assert!(!channel_error_kind(&error).contains("sensitive"));
}
