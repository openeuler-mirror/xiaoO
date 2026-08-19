//! Backend 管理面与进程守卫。
//!
//! 【L1 支撑类型层】
//!
//! ## xiaoo 侧用到的 BackendManager 方法面
//!
//! - `new()` / `new_with_storage_dir(...)`：构造
//! - `start_signal_handler(session_store)`：跨进程 SIGUSR1 驱逐协作
//!   （常驻进程如 TUI 需要；单次运行的 CLI 不需要）
//! - `release_session(session_id)`：`/new` 时释放绑定
//! - `shutdown_all()`：退出收尾
//!
//! 常规路径全部由门面代劳（[`crate::host::LocalSessionHostBuilder::backend_manager`]
//! 与 [`crate::host::LocalSessionHost::shutdown`]，阶段 2 落地）；直接触碰
//! 属深度集成。
//!
//! 其余管理面（`list`、`BackendInfo`、`BackendListFilter`、E2B bootstrap 工具族）
//! 是 daemon 独有，不导出。
//!
//! ## GatewayBackendConfig
//!
//! [`GatewayBackendConfig::new(kind, options)`]：`kind ∈ {"local", "e2b"}`，
//! `options` 为 provider 专属键值（E2B 的 api_key_env / template 等）。xiaoo
//! 的 sandbox 切换对话框用它构造，经 [`crate::host::SessionOptions::backend`]
//! 传入。

#[doc(inline)]
pub use xiaoo_shared::backend::{BackendManager, GatewayBackendConfig};

// ---- 进程守卫：双方 main.rs 都用 ----
#[doc(inline)]
pub use operation_backend::process_group::ProcessGroupCleanupGuard;
