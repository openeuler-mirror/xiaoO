use serde::Serialize;

use crate::channels::{FeishuEventTransport, TelegramEventTransport};
use crate::daemon_config::DaemonConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChannelIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChannelStatus {
    pub id: String,
    pub configured: bool,
    pub enabled: bool,
    pub transport: String,
    pub channel_instance_id: Option<String>,
    pub identity: Option<String>,
    pub base_url: String,
    pub webhook_path: Option<String>,
    pub credential_env: Option<String>,
    pub credential_available: bool,
    pub verification_available: bool,
    pub polling_timeout_secs: Option<u64>,
    pub polling_limit: Option<u16>,
    pub valid: bool,
    pub errors: Vec<ChannelIssue>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChannelReport {
    pub schema_version: u32,
    pub interaction_timeout_secs: u64,
    pub channels: Vec<ChannelStatus>,
}

pub fn channel_report(config: &DaemonConfig) -> ChannelReport {
    ChannelReport {
        schema_version: 1,
        interaction_timeout_secs: config.interaction_timeout_secs(),
        channels: vec![feishu_status(config), telegram_status(config)],
    }
}

fn feishu_status(config: &DaemonConfig) -> ChannelStatus {
    let Some(raw) = config.app.channels.feishu.as_ref() else {
        return ChannelStatus {
            id: "feishu".to_string(),
            configured: false,
            enabled: false,
            transport: "webhook".to_string(),
            channel_instance_id: None,
            identity: None,
            base_url: "https://open.feishu.cn".to_string(),
            webhook_path: Some("/api/v1/channels/feishu/events".to_string()),
            credential_env: None,
            credential_available: false,
            verification_available: false,
            polling_timeout_secs: None,
            polling_limit: None,
            valid: true,
            errors: Vec::new(),
        };
    };
    let credential_env = normalized(raw.app_secret_env.as_deref());
    let credential_available = credential_env.as_deref().is_some_and(env_available);
    let mut errors = Vec::new();
    if raw.enabled {
        if let Err(error) = config.feishu_config() {
            issue(&mut errors, "channels.feishu", "invalid_config", error);
        }
        if credential_env.is_some() && !credential_available {
            issue_message(
                &mut errors,
                "channels.feishu.app_secret_env",
                "missing_secret",
                "引用的飞书 App Secret 环境变量未设置",
            );
        }
    }
    let transport = match raw.event_transport {
        FeishuEventTransport::Webhook => "webhook",
        FeishuEventTransport::Websocket => "websocket",
    };
    ChannelStatus {
        id: "feishu".to_string(),
        configured: true,
        enabled: raw.enabled,
        transport: transport.to_string(),
        channel_instance_id: normalized(raw.channel_instance_id.as_deref()),
        identity: normalized(raw.app_id.as_deref()),
        base_url: normalized(raw.base_url.as_deref())
            .unwrap_or_else(|| "https://open.feishu.cn".to_string()),
        webhook_path: (raw.event_transport == FeishuEventTransport::Webhook)
            .then(|| "/api/v1/channels/feishu/events".to_string()),
        credential_env,
        credential_available,
        verification_available: normalized(raw.verification_token.as_deref()).is_some(),
        polling_timeout_secs: None,
        polling_limit: None,
        valid: errors.is_empty(),
        errors,
    }
}

fn telegram_status(config: &DaemonConfig) -> ChannelStatus {
    let Some(raw) = config.app.channels.telegram.as_ref() else {
        return ChannelStatus {
            id: "telegram".to_string(),
            configured: false,
            enabled: false,
            transport: "webhook".to_string(),
            channel_instance_id: None,
            identity: None,
            base_url: "https://api.telegram.org".to_string(),
            webhook_path: Some("/api/v1/channels/telegram/events".to_string()),
            credential_env: None,
            credential_available: false,
            verification_available: false,
            polling_timeout_secs: Some(50),
            polling_limit: Some(100),
            valid: true,
            errors: Vec::new(),
        };
    };
    let credential_env = normalized(raw.bot_token_env.as_deref());
    let credential_available = credential_env.as_deref().is_some_and(env_available);
    let mut errors = Vec::new();
    if raw.enabled {
        if let Err(error) = config.telegram_config() {
            issue(&mut errors, "channels.telegram", "invalid_config", error);
        }
        if credential_env.is_some() && !credential_available {
            issue_message(
                &mut errors,
                "channels.telegram.bot_token_env",
                "missing_secret",
                "引用的 Telegram Bot Token 环境变量未设置",
            );
        }
    }
    let transport = match raw.event_transport {
        TelegramEventTransport::Webhook => "webhook",
        TelegramEventTransport::Polling => "polling",
    };
    ChannelStatus {
        id: "telegram".to_string(),
        configured: true,
        enabled: raw.enabled,
        transport: transport.to_string(),
        channel_instance_id: normalized(raw.channel_instance_id.as_deref()),
        identity: normalized(raw.bot_username.as_deref()),
        base_url: normalized(raw.base_url.as_deref())
            .unwrap_or_else(|| "https://api.telegram.org".to_string()),
        webhook_path: (raw.event_transport == TelegramEventTransport::Webhook)
            .then(|| "/api/v1/channels/telegram/events".to_string()),
        credential_env,
        credential_available,
        verification_available: normalized(raw.webhook_secret_token.as_deref()).is_some(),
        polling_timeout_secs: Some(raw.polling_timeout_secs.unwrap_or(50)),
        polling_limit: Some(raw.polling_limit.unwrap_or(100)),
        valid: errors.is_empty(),
        errors,
    }
}

fn normalized(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn env_available(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

fn issue(errors: &mut Vec<ChannelIssue>, path: &str, code: &str, error: anyhow::Error) {
    issue_message(errors, path, code, &error.to_string());
}

fn issue_message(errors: &mut Vec<ChannelIssue>, path: &str, code: &str, message: &str) {
    errors.push(ChannelIssue {
        path: path.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::channel_report;
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
}
