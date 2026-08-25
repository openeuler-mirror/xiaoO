//! MCP 配置词汇再导出。
//!
//! 应用层（endside 配置解析、serverside daemon_config / channel_runtime）的
//! serde 配置字段需要命名 [`McpSection`] / [`McpServerConfig`] 才能从配置文件
//! 反序列化与持有，无法下沉为粗粒度函数（应用必须命名该类型），故作为配置
//! 词汇再导出。其余 mcp 内部实现类型不向应用导出。

pub use mcp::{McpSection, McpServerConfig};
