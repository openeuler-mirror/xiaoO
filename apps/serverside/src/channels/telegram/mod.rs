mod channel;
mod client;
mod polling;
mod types;

pub use channel::{capabilities, meta, TelegramAdapter};
pub(crate) use client::TelegramClient;
pub use polling::{TelegramPollingMessageHandler, TelegramPollingService};
pub use types::{TelegramConfig, TelegramEventTransport};
