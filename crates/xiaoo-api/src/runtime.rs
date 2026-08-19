//! 装配契约与 wire 协议。
//!
//! 本模块分两层：
//!
//! - **【L2 Advanced】装配契约**：把"配置 → 可运行 runtime"的扩展点与
//!   宿主注入回调的载体收敛到这里。门面用户不接触本模块（[`crate::host`]
//!   与 [`crate::session`] 内部代劳）；保留导出供测试替身与特殊装配场景。
//! - **【L1】wire 协议**：[`wire`] 子模块是 xiaoo（HTTP client）访问远端
//!   daemon（已迁往独立代码仓）的序列化协议面，未来可作为两仓共同依赖的
//!   协议契约包。
//!
//! 原定义保持 `pub` 不变；re-export 不产生新类型，trait impl / serde 全部兼容。

pub mod wire;

// ---- 【L2 Advanced】装配契约 ----

/// `AppBootstrap` 与 [`AppDependencies`]：把 store / secret / automation /
/// resolver / bootstrap 五步组装成 [`AppDependencies`]。门面的
/// `LocalSessionHostBuilder::build()` 内部调用它。
#[doc(inline)]
pub use xiaoo_shared::gateway::bootstrap::{AppBootstrap, AppBootstrapError, AppDependencies};

/// Hosted 配置与 resolver：xiaoo 侧装配 runtime 的标准实现。
#[doc(inline)]
pub use xiaoo_shared::gateway::hosted_runtime_resolver::{
    HostedSessionRuntimeConfig, HostedSessionRuntimeResolver, SubagentRoleConfigEntry,
};

/// "配置 → 可运行 runtime" 的扩展点。shared 的 [`HostedSessionRuntimeResolver`]
/// 是其标准实现（daemon 仓可自实现该 trait）。
#[doc(inline)]
pub use xiaoo_shared::gateway::session_runtime::{
    ResolvedSessionRuntime, SessionRuntimeBindings, SessionRuntimeBuildInput,
    SessionRuntimeDescriptor, SessionRuntimeResolveError, SessionRuntimeResolver,
};

/// 内核 kv-cache 预取；本地与远程（经 shared 间接）两侧共用。
#[doc(inline)]
pub use xiaoo_core::spawn_prefetch;

/// 用户在 turn 进行中追加排队的输入源；与 [`crate::host::TurnHandle::queue_input`]
/// 对应。
#[doc(inline)]
pub use xiaoo_core::PendingUserMessageSource;
