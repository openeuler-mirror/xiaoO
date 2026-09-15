use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use xiaoo_shared::daemon_protocol::response::{
    ChannelCatalogResponse, ChannelRuntimeResponse, ChannelTestResponse,
};

use crate::channels::feishu::FeishuClient;
use crate::channels::telegram::TelegramClient;
use crate::channels::{
    ChannelError, FeishuConfig, FeishuEventTransport, TelegramConfig, TelegramEventTransport,
};
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

#[derive(Debug, Clone)]
struct ChannelRuntimeState {
    transport: String,
    received_count: u64,
    success_count: u64,
    failure_count: u64,
    last_received_at_ms: Option<u64>,
    last_completed_at_ms: Option<u64>,
    last_error_kind: Option<String>,
}

/// Secret-free runtime status and connection tests for enabled channels.
pub struct ChannelManager {
    states: Mutex<HashMap<String, ChannelRuntimeState>>,
    feishu: Option<FeishuConfig>,
    telegram: Option<TelegramConfig>,
}

impl ChannelManager {
    pub fn new(feishu: Option<FeishuConfig>, telegram: Option<TelegramConfig>) -> Self {
        let mut states = HashMap::new();
        if let Some(config) = feishu.as_ref() {
            states.insert(
                "feishu".to_string(),
                ChannelRuntimeState::new(match config.event_transport {
                    FeishuEventTransport::Webhook => "webhook",
                    FeishuEventTransport::Websocket => "websocket",
                }),
            );
        }
        if let Some(config) = telegram.as_ref() {
            states.insert(
                "telegram".to_string(),
                ChannelRuntimeState::new(match config.event_transport {
                    TelegramEventTransport::Webhook => "webhook",
                    TelegramEventTransport::Polling => "polling",
                }),
            );
        }
        Self {
            states: Mutex::new(states),
            feishu,
            telegram,
        }
    }

    pub fn catalog(&self) -> ChannelCatalogResponse {
        let states = self.states.lock().expect("channel state mutex poisoned");
        let mut channels = states
            .iter()
            .map(|(id, state)| state.response(id))
            .collect::<Vec<_>>();
        channels.sort_by(|left, right| left.id.cmp(&right.id));
        ChannelCatalogResponse { channels }
    }

    pub fn record_received(&self, id: &str) {
        if let Some(state) = self
            .states
            .lock()
            .expect("channel state mutex poisoned")
            .get_mut(id)
        {
            state.received_count = state.received_count.saturating_add(1);
            state.last_received_at_ms = Some(now_ms());
        }
    }

    pub fn record_success(&self, id: &str) {
        if let Some(state) = self
            .states
            .lock()
            .expect("channel state mutex poisoned")
            .get_mut(id)
        {
            state.success_count = state.success_count.saturating_add(1);
            state.last_completed_at_ms = Some(now_ms());
            state.last_error_kind = None;
        }
    }

    pub fn record_failure(&self, id: &str, kind: &str) {
        if let Some(state) = self
            .states
            .lock()
            .expect("channel state mutex poisoned")
            .get_mut(id)
        {
            state.failure_count = state.failure_count.saturating_add(1);
            state.last_completed_at_ms = Some(now_ms());
            state.last_error_kind = Some(kind.to_string());
        }
    }

    pub async fn test_connection(&self, id: &str) -> Option<ChannelTestResponse> {
        let started = Instant::now();
        let result = match id {
            "feishu" => match self.feishu.clone() {
                Some(config) => {
                    let client = FeishuClient::new(config);
                    tokio::time::timeout(Duration::from_secs(15), client.test_connection())
                        .await
                        .unwrap_or_else(|_| {
                            Err(ChannelError::Transport {
                                message: "request timed out".to_string(),
                            })
                        })
                }
                None => return None,
            },
            "telegram" => match self.telegram.clone() {
                Some(config) => {
                    let client = TelegramClient::new(config);
                    tokio::time::timeout(Duration::from_secs(15), client.test_connection())
                        .await
                        .unwrap_or_else(|_| {
                            Err(ChannelError::Transport {
                                message: "request timed out".to_string(),
                            })
                        })
                }
                None => return None,
            },
            _ => return None,
        };
        let error_kind = result
            .as_ref()
            .err()
            .map(channel_error_kind)
            .map(str::to_string);
        Some(ChannelTestResponse {
            id: id.to_string(),
            success: result.is_ok(),
            duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            error_kind,
        })
    }
}

impl ChannelRuntimeState {
    fn new(transport: &str) -> Self {
        Self {
            transport: transport.to_string(),
            received_count: 0,
            success_count: 0,
            failure_count: 0,
            last_received_at_ms: None,
            last_completed_at_ms: None,
            last_error_kind: None,
        }
    }

    fn response(&self, id: &str) -> ChannelRuntimeResponse {
        ChannelRuntimeResponse {
            id: id.to_string(),
            transport: self.transport.clone(),
            loaded: true,
            received_count: self.received_count,
            success_count: self.success_count,
            failure_count: self.failure_count,
            last_received_at_ms: self.last_received_at_ms,
            last_completed_at_ms: self.last_completed_at_ms,
            last_error_kind: self.last_error_kind.clone(),
        }
    }
}

pub fn channel_error_kind(error: &ChannelError) -> &'static str {
    match error {
        ChannelError::Config { .. } => "configuration",
        ChannelError::InvalidEvent { .. } => "invalid_event",
        ChannelError::Authentication { .. } => "authentication",
        ChannelError::Transport { .. } => "transport",
        ChannelError::Delivery { .. } => "delivery",
        ChannelError::UnsupportedCapability { .. } => "unsupported_capability",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
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
#[path = "../../../tests/unit/serverside/channel_management_test.rs"]
mod tests;
