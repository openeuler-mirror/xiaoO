pub mod adapter;
pub mod feishu;
pub mod telegram;

pub use adapter::{
    AdapterResponse, ChannelAdapter, ChannelAttachment, ChannelCapabilities, ChannelError,
    ChannelMember, ChannelMention, ChannelMessage, ChannelMeta, ChannelOutboundAttachment,
    ChannelOutboundAttachmentKind, ChannelProgressSection, ChannelProgressState,
    ChannelProgressUpdate, ChannelResult, ChannelRuntime, ChannelTextFormat,
};
pub use feishu::{
    capabilities as feishu_capabilities, meta as feishu_meta, FeishuAdapter, FeishuCardRequest,
    FeishuChatInfo, FeishuChatMember, FeishuClient, FeishuConfig, FeishuConfigError,
    FeishuEventTransport, FeishuSendRequest, FeishuWebsocketMessageHandler, FeishuWebsocketService,
};
pub use telegram::{
    capabilities as telegram_capabilities, meta as telegram_meta, TelegramAdapter, TelegramClient,
    TelegramConfig, TelegramConfigError, TelegramEventTransport, TelegramPollingMessageHandler,
    TelegramPollingService,
};

pub fn build_feishu_runtime(config: FeishuConfig) -> ChannelResult<ChannelRuntime> {
    let instance_id = config
        .channel_instance_id
        .clone()
        .unwrap_or_else(|| "feishu".to_string());
    let adapter =
        std::sync::Arc::new(FeishuAdapter::new(config)?) as std::sync::Arc<dyn ChannelAdapter>;
    Ok(ChannelRuntime {
        instance_id,
        channel_id: "feishu".to_string(),
        meta: feishu_meta(),
        capabilities: feishu_capabilities(),
        adapter,
    })
}

pub fn build_telegram_runtime(config: TelegramConfig) -> ChannelResult<ChannelRuntime> {
    let instance_id = config
        .channel_instance_id
        .clone()
        .unwrap_or_else(|| "telegram".to_string());
    let adapter =
        std::sync::Arc::new(TelegramAdapter::new(config)?) as std::sync::Arc<dyn ChannelAdapter>;
    Ok(ChannelRuntime {
        instance_id,
        channel_id: "telegram".to_string(),
        meta: telegram_meta(),
        capabilities: telegram_capabilities(),
        adapter,
    })
}
