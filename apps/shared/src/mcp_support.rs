//! MCP 配置词汇与配置合并工具再导出。
//!
//! 应用层（endside support/config、serverside daemon_config）需要命名
//! [`McpSection`] / [`McpServerConfig`] 以 serde 反序列化与持有配置字段，
//! 并复用 [`resolve_json_config_path`] / [`load_json_servers`] /
//! [`merge_server_configs`] 把 toml + json 两路 MCP 配置合并为运行时
//! 服务器列表——无法下沉为粗粒度函数（应用必须命名这些类型与函数签名），
//! 故作为配置词汇与配套函数再导出。`McpConfigError` 是这些函数的错误类型，
//! 随函数签名可达。其余 mcp 内部实现类型不向应用导出。
//!
//! 待办：endside 与 serverside 各自复刻了 `load_merged_mcp_servers` 近似
//! 副本，可进一步合并为 shared 单一粗粒度入口；届时这些函数的再导出可
//! 移除。

pub use mcp::{
    load_json_servers, merge_server_configs, resolve_json_config_path, McpConfigError, McpSection,
    McpServerConfig, Transport,
};
