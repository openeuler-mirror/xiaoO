//! 会话文件 diff 模型与解析。
//!
//! 【L1 支撑类型层】[`SessionDiffTracker`] 把工具生命周期事件转换为文件
//! 增删统计；本地与远程模式逻辑一致，远程模式下 daemon 序列化计算好的
//! `FileChangeDelta` 通过 SSE 转发，本地模式直接调用 tracker。
//!
//! daemon 侧的 `DiffComputingLoopSink` / `DiffComputingToolSink` / `SessionDiffForwarder`
//! 等 `daemon_sinks.rs` 内容属 daemon 独有（`refactor.md` §3.5），不进入本模块。

#[doc(inline)]
pub use xiaoo_shared::session_diff::{
    file_change_delta_from_tool_args, parse_tool_target_file_path, FileChangeDelta,
    SessionDiffTracker, SessionFileChangeEntry, SessionFileChangeStats,
};
