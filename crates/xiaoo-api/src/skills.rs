//! skills 注册表与审计。
//!
//! 【L2 Advanced 层】
//!
//! [`SkillRegistry`]（trait）+ [`FileSkillRegistry`]（文件系统实现）+
//! [`audit_skill_directory`] / [`SkillAuditOptions`]（目录审计）是 skills
//! 注册表的核心扩展点。常规路径下，skills 目录的四级优先级解析（
//! `support/config.rs:330-370` 与 `cli/entry.rs:340-436` 的两份合一）已
//! 由门面 [`crate::host::SessionOptions`] 派生内部调用（`refactor.md`
//! §3.3.9）；本模块保留导出供深度定制与测试。

#[doc(inline)]
pub use agent_contracts::SkillRegistry;

#[doc(inline)]
pub use skill::{
    audit::{audit_skill_directory, SkillAuditOptions},
    FileSkillRegistry,
};
