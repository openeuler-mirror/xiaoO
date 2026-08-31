pub mod adapter;

pub use adapter::{
    AdapterResponse, ChannelAdapter, ChannelAttachment, ChannelCapabilities, ChannelError,
    ChannelMember, ChannelMention, ChannelMessage, ChannelMeta, ChannelOutboundAttachment,
    ChannelOutboundAttachmentKind, ChannelProgressSection, ChannelProgressState,
    ChannelProgressUpdate, ChannelResult, ChannelRuntime, ChannelTextFormat,
};

// 契约句柄再导出：serverside 的 service / channel_runtime 持有
// `Option<Arc<dyn ChannelFileSender>>` 字段并 impl `AdapterFileSender`，
// 必须命名该 trait，故再导出。
pub use agent_contracts::ChannelFileSender;
