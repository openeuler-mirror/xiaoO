//! LSP 服务注册表与 server 配置。
//!
//! 【L1 支撑类型层】[`LspServiceRegistry`] 是 LSP 服务表，[`ServerConfig`]
//! 描述单个 LSP server 启动方式，[`AutoInstall`] 控制是否自动安装 LSP
//! 二进制。门面 [`crate::host::SessionOptions::lsp`]（阶段 2 落地）接收一个
//! [`LspServiceRegistry`]；缺省不启用。

#[doc(inline)]
pub use lsp::{AutoInstall, LspServiceRegistry, ServerConfig};
