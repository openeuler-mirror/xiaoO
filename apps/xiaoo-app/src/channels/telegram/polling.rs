use crate::channels::telegram::client::TelegramClient;
use crate::channels::telegram::types::{TelegramConfig, TelegramEventTransport};
use crate::channels::telegram::TelegramAdapter;
use crate::channels::{ChannelError, ChannelMessage, ChannelResult};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

pub type TelegramPollingMessageHandler =
    Arc<dyn Fn(ChannelMessage) -> BoxFuture<'static, ()> + Send + Sync>;

pub struct TelegramPollingService {
    config: TelegramConfig,
    adapter: TelegramAdapter,
    client: TelegramClient,
}

impl TelegramPollingService {
    pub fn new(config: TelegramConfig) -> ChannelResult<Self> {
        if config.event_transport != TelegramEventTransport::Polling {
            return Err(ChannelError::Config {
                message: "telegram polling service requires polling transport".to_string(),
            });
        }
        let adapter = TelegramAdapter::new(config.clone())?;
        Ok(Self {
            client: TelegramClient::new(config.clone()),
            config,
            adapter,
        })
    }

    pub async fn run_forever(self, handler: TelegramPollingMessageHandler) {
        let mut offset = None;
        loop {
            match self
                .client
                .get_updates(
                    offset,
                    self.config.polling_timeout_secs,
                    self.config.polling_limit,
                )
                .await
            {
                Ok(updates) => {
                    for update in updates {
                        let next_offset = update.update_id + 1;
                        match self.adapter.handle_update(update) {
                            Ok(Some(message)) => {
                                let handler = handler.clone();
                                tokio::spawn(async move {
                                    (handler)(message).await;
                                });
                            }
                            Ok(None) => {}
                            Err(error) => warn!("failed to parse telegram polling update: {error}"),
                        }
                        offset = Some(next_offset);
                    }
                }
                Err(error) => {
                    warn!("telegram polling request failed: {error}");
                    sleep(Duration::from_secs(5)).await;
                }
            }
            debug!(?offset, "telegram polling cycle completed");
        }
    }
}
