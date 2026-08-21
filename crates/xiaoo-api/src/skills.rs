//! skills 注册表与审计。
//!
//! [`SkillRegistry`]（trait）+ [`FileSkillRegistry`]（文件系统实现）+
//! [`audit_skill_directory`] / [`SkillAuditOptions`]（目录审计）构成 runtime
//! 的 skill 注入面。

#[doc(inline)]
pub use agent_contracts::SkillRegistry;

#[doc(inline)]
pub use xiaoo_core::EmptySkillRegistry;

#[doc(inline)]
pub use skill::{
    audit::{audit_skill_directory, SkillAuditOptions},
    FileSkillRegistry,
};
