//! TODO / 子代理计划模型与解析。
//!
//! 【L1 支撑类型层】daemon 在远程模式预计算 `PlanUpdate` SSE 事件并转发给
//! xiaoo；本地模式无结构化 plan 事件，需要 plan 的调用方从 `todo_write` 工具
//! 的 `args_preview` JSON 反解析——本模块的 [`todo_snapshot_from_tool_args`]
//! 即是该解析路径。`refactor.md` §7.4 治理后由内核直接产出，届时删除反解析路径。
//!
//! daemon 侧的 `PlanComputingLoopSink` / `SubagentMetaComputingLoopSink` 等
//! 装饰器属 daemon 独有（`refactor.md` §3.5），不进入本模块。

#[doc(inline)]
pub use xiaoo_shared::plan::{
    parse_spawn_subagent_agent_id_from_detail, parse_spawn_subagent_metadata_from_args,
    todo_snapshot_from_tool_args, SpawnSubagentMetadata, TodoDisplayStatus, TodoSnapshotItem,
    TodoSnapshotUpdate,
};
