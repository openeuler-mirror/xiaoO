use super::{FeishuConfig, FeishuEventTransport};

fn config(event_transport: FeishuEventTransport, verification_token: Option<&str>) -> FeishuConfig {
    FeishuConfig {
        channel_instance_id: Some("ops-feishu".to_string()),
        base_url: "https://open.feishu.cn".to_string(),
        app_id: "cli_test".to_string(),
        app_secret_env: "FEISHU_APP_SECRET".to_string(),
        event_transport,
        verification_token: verification_token.map(ToString::to_string),
        parse_file_messages: false,
        max_file_download_bytes: 0,
        max_file_text_chars: 0,
    }
}

#[test]
fn webhook_transport_requires_verification_token() {
    let error = config(FeishuEventTransport::Webhook, None)
        .validate()
        .expect_err("webhook transport should require a verification token");

    assert!(matches!(
        error,
        super::FeishuConfigError::InvalidField {
            field: "verification_token",
            ..
        }
    ));
}

#[test]
fn websocket_transport_allows_missing_verification_token() {
    config(FeishuEventTransport::Websocket, None)
        .validate()
        .expect("websocket transport should not require a verification token");
}
