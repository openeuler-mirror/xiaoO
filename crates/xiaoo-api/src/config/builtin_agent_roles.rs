//! 内置 agent 角色（plan 子代理）。
//!
//! 【L1 支撑类型层】[`PLAN_AGENT_ID`] / [`PLAN_AGENT_DESCRIPTION`] /
//! [`PLAN_AGENT_PROMPT`] 是 xiaoo 内置的只读 plan 子代理的标识、描述与
//! prompt。门面 [`crate::host::SessionOptions`] 派生 subagent 角色时默认
//! 包含 plan 角色（`refactor.md` §3.3.3 派生规则表）。

#[doc(inline)]
pub use xiaoo_shared::builtin_agent_roles::{
    PLAN_AGENT_DESCRIPTION, PLAN_AGENT_ID, PLAN_AGENT_PROMPT,
};
