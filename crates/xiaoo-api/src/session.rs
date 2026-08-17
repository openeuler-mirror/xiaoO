//! 会话服务契约：任何 xiaoo 入口（TUI/CLI/HTTP/IM/cron/MCP）驱动 Agent 会话的统一接口。
//!
//! 【L1 支撑类型层】常规调用方使用 [`crate::host`] 门面即可；本模块供远程协议、
//! advanced 装配与 daemon 未来复用。trait 本体经门面访问器使用，不进入 [`crate::prelude`]。
//!
//! 原定义保持 `pub` 不变，本模块仅做 `pub use` 重组路径，不产生新类型。
//! 契约语义与默认实现以 `apps/shared/src/gateway/session_service*.rs` 为准。

// ---- 会话服务 trait 与错误 ----
#[doc(inline)]
pub use xiaoo_shared::gateway::session_service::{
    SessionControlPlane, SessionService, SessionServiceError,
};

// ---- 会话存储（实现偏 L2：门面用户经 host.session_store() 取得）----
#[doc(inline)]
pub use xiaoo_shared::gateway::session_store::{
    InMemorySessionStore, SessionStore, SessionStoreError,
};

// ---- turn/会话 DTO ----
#[doc(inline)]
pub use xiaoo_shared::gateway::session_base::{
    channel_session_id, SessionCancelRequest, SessionCloseRequest, SessionDetachRequest,
    SessionForkRequest, SessionForkResult, SessionHeartbeatRequest, SessionInput,
    SessionInputKind, SessionInteractionRequest, SessionOpenRequest, SessionSubmitReceipt,
};

#[doc(inline)]
pub use xiaoo_shared::gateway::session_record::{
    SessionAgentRecord, SessionLifecycleStatus, SessionRecord, SessionRuntimeSnapshot,
    SessionStateOutcome, SubagentRoleRecord,
};

#[doc(inline)]
pub use xiaoo_shared::gateway::turns::{
    AppTurnRequest, AppTurnResult, GatewayEntryContext, GatewayEntryKind, LlmRuntimeConfig,
    TurnMention, TurnOutcome,
};
