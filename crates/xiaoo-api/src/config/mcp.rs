//! MCP 配置件与 JSON 配置文件解析。
//!
//! 【L1 支撑类型层】[`McpSection`] 是 TOML `[mcp]` 段模型；[`McpServerConfig`]
//! 描述单个 MCP server；[`resolve_json_config_path`] + [`load_json_servers`] +
//! [`merge_server_configs`] 是四级优先级解析链的 helper——这些目前是
//! 调用方必须手工调用的公开函数，重构后全部变成 [`crate::host::SessionOptions`]
//! 派生过程的内部实现（`refactor.md` §3.3.9）。

#[doc(inline)]
pub use mcp::{
    load_json_servers, merge_server_configs, resolve_json_config_path, McpConfigError, McpSection,
    McpServerConfig, Transport,
};
