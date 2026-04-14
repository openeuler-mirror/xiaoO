pub mod adapter;
pub mod feishu;

pub use adapter::{
    AdapterResponse, ChannelAdapter, ChannelAttachment, ChannelCapabilities, ChannelError,
    ChannelMember, ChannelMention, ChannelMessage, ChannelMeta, ChannelOutboundAttachment,
    ChannelOutboundAttachmentKind, ChannelProgressSection, ChannelProgressState,
    ChannelProgressUpdate, ChannelResult, ChannelRuntime, ChannelTextFormat,
};
pub use feishu::{
    capabilities as feishu_capabilities, meta as feishu_meta, FeishuAdapter, FeishuCardRequest,
    FeishuChatInfo, FeishuChatMember, FeishuClient, FeishuConfig, FeishuConfigError,
    FeishuSendRequest,
};
